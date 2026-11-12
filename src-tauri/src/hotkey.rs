//! 全局快捷键与"唤出 / 收走"生命周期。
//!
//! 产品形态是**召唤式**：窗口默认隐藏，热键唤出，记完或 Esc 收走。
//! 本模块只处理"窗口怎么出现、出现在哪、什么时候消失"，不碰任何业务判断。

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use slip_app::geometry::{Area, place};
use tauri::{App, AppHandle, Emitter, Manager, PhysicalPosition, WindowEvent};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

/// 默认热键。选 `Ctrl+Alt+Space`：不占 `Alt+Space`（系统菜单），
/// 也不与常见粘贴板 / 输入法快捷键冲突。
const DEFAULT_SHORTCUT: &str = "ctrl+alt+space";

/// 前端监听此事件来把光标放进捕获框并清掉提示。
const SUMMON_EVENT: &str = "slip://summon";

/// 唤出后多久之内收到的"失焦"事件要忽略（规则本体在 `slip_app::focus`）。
const SUMMON_GRACE: Duration = slip_app::focus::SUMMON_GRACE;

/// 窗口与光标之间留的缝（逻辑像素）。
const CURSOR_GAP: f64 = 16.0;

static LAST_SUMMON: Mutex<Option<Instant>> = Mutex::new(None);

/// 本次唤出之后，窗口**有没有真的被聚焦过**。
///
/// 为什么需要它：手动双击启动时，Windows 的前台锁定常常不把前台给新窗口，
/// `set_focus()` 于是失败、系统立刻送来 `Focused(false)`，窗口在用户看到之前
/// 就被收走了——双击 exe 又变成"闪一下就没了"。
/// 从未被聚焦过的窗口不存在"用户正在用它却失去了焦点"，因此不该被失焦规则收走。
static FOCUSED_SINCE_SUMMON: AtomicBool = AtomicBool::new(false);

/// 失焦自动收走是否「上膛」。
///
/// 为什么需要这个开关：启动展示与热键召唤是两种不同的意图。
/// 热键召唤是"我喊它出来"，点一下别处就该收走；而双击启动是"我要打开它"，
/// 此时如果别的常驻程序（Clash Verge、输入法、更新弹窗都会这么干）抢走前台，
/// 窗口就会被失焦规则收走——用户看到的是"窗口闪一下，然后怎么都找不到"。
/// 实测本机 Clash Verge 每约 2.7 秒抢一次前台，窗口必然消失。
///
/// 所以：启动展示不上膛，只有热键/托盘召唤才上膛。
/// 启动出来的窗口靠 `Esc`、托盘或热键收走——它本来就是个正常窗口。
static AUTO_DISMISS_ARMED: AtomicBool = AtomicBool::new(true);

fn note_summon() {
    if let Ok(mut slot) = LAST_SUMMON.lock() {
        *slot = Some(Instant::now());
    }
}

/// 把配置里的字符串解析成一个**确定可用**的热键文本。
///
/// 存储层只存不透明字符串（它不该认识键名文法），所以合法性判定落在这里。
/// 解析失败不能让应用起不来——回落到默认值并留一条 warn。
pub fn resolve_shortcut(configured: Option<&str>) -> String {
    match configured {
        Some(text) => match text.trim().parse::<Shortcut>() {
            Ok(_) => text.trim().to_ascii_lowercase(),
            Err(error) => {
                tracing::warn!(configured = %text, %error, "配置的热键无法解析，回落到默认值");
                DEFAULT_SHORTCUT.to_owned()
            }
        },
        None => DEFAULT_SHORTCUT.to_owned(),
    }
}

pub fn register(app: &App, text: &str) -> Result<(), Box<dyn std::error::Error>> {
    // 失焦收走与热键注册无关：注册失败时托盘仍能显示窗口，也必须能自动收起。
    install_auto_dismiss(app);

    let shortcut: Shortcut = text.parse()?;
    bind(app.handle(), shortcut)?;
    tracing::info!("hotkey registered: {text}");
    Ok(())
}

/// 把一个快捷键绑成"按下即切换窗口"。
///
/// 与 [`register`] 分开：启动注册要连带装失焦收走，而运行期换键（R116）只该动注册，
/// 不能重复安装窗口事件监听（那会让一次失焦被处理两遍）。
fn bind(handle: &AppHandle, shortcut: Shortcut) -> Result<(), String> {
    handle
        .global_shortcut()
        .on_shortcut(shortcut, |app, _shortcut, event| {
            // 只响应按下：否则一次按键会触发 Pressed + Released 两次。
            if event.state() == ShortcutState::Pressed {
                toggle(app);
            }
        })
        .map_err(|error| error.to_string())
}

/// 备选热键（R116）：默认键被别的程序占用时的第二选择。
///
/// 选 `Win+Alt+S`：`S` 取自 Slip，且 Win+Alt 组合极少被普通应用注册。
pub const FALLBACK_SHORTCUT: &str = "super+alt+s";

/// 一次"换备选键"尝试的三种结局。
///
/// 为什么不是 `Result<String, String>`：失败与"没换成但旧键还在"是两件不同的事。
/// 用 `Err` 表达后者会让调用方分不清"该不该把界面上的热键置空"，
/// 于是出现"提示说失败、界面仍显示旧键可用"——这正是要修的骗人状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FallbackOutcome {
    /// 备选键注册成功；携带真正生效的键文本。
    Switched(String),
    /// 备选键注册失败，但旧键已重新注册；携带旧键文本。
    RestoredOld(String),
    /// 备选键与旧键都注册失败：此刻**没有**任何全局热键。
    NoneRegistered,
}

/// 这次尝试之后，托管状态里应当写的键。
///
/// 纯函数，三态到"有没有热键"的映射只能在这里定义一次：
/// 调用方直接拿返回值写状态，没有机会"只更新成功分支、失败分支留着旧值"。
/// `None` 表示必须置空——界面宁可显示"未注册"，也不能显示一个按不响的键。
pub fn next_hotkey(outcome: &FallbackOutcome, current: &str) -> Option<String> {
    match outcome {
        FallbackOutcome::Switched(key) => Some(key.clone()),
        // 旧键被重新注册，状态与切换前一致。
        FallbackOutcome::RestoredOld(_) => Some(current.to_owned()),
        FallbackOutcome::NoneRegistered => None,
    }
}

/// 把当前热键换成 [`FALLBACK_SHORTCUT`]。
///
/// 三条纪律：
/// - **先解析再注销**：新键写错时旧键还没被动过，不会出现"换失败且原来也没了"；
/// - 失败**尽力恢复**旧键；恢复也失败就返回 [`FallbackOutcome::NoneRegistered`]，
///   不假称成功、也不假装旧键还在；
/// - **不在这里写偏好**：落盘是外壳的职责（`system::persist_hotkey`），
///   且只有 [`FallbackOutcome::Switched`] 才写——本模块不认识 vault 路径。
///
/// 解析备选键或注销旧键失败仍是 `Err`：前者没动过注册，后者连"重来"的起点都没有，
/// 两者都不该被当成一次有效的切换尝试。
/// 换键时的候选顺序：备选 → 旧键（若非空）→ 默认键（若与前面不同）。
///
/// 为什么需要默认键兜底：上次持久化了备选键，下次登录它也被占用时，
/// 只重试备选键会永远失败——即使默认键此刻已经释放，本次会话仍然没有热键，
/// 用户只能手改 `ui-state.json`。纯函数，顺序可断言。
fn fallback_candidates(current: &str) -> Vec<String> {
    let current = current.trim();
    let mut candidates = vec![FALLBACK_SHORTCUT.to_owned()];
    if !current.is_empty() && current != FALLBACK_SHORTCUT {
        candidates.push(current.to_owned());
    }
    if current != DEFAULT_SHORTCUT && FALLBACK_SHORTCUT != DEFAULT_SHORTCUT {
        candidates.push(DEFAULT_SHORTCUT.to_owned());
    }
    candidates
}

pub fn switch_to_fallback(handle: &AppHandle, current: &str) -> Result<FallbackOutcome, String> {
    let candidates = fallback_candidates(current);
    handle
        .global_shortcut()
        .unregister_all()
        .map_err(|error| format!("注销旧热键失败：{error}"))?;

    let mut last_error = String::new();
    for candidate in candidates {
        let Ok(shortcut) = candidate.parse::<Shortcut>() else {
            last_error = format!("热键无法解析：{candidate}");
            continue;
        };
        match bind(handle, shortcut) {
            Ok(()) => {
                tracing::info!("hotkey switched to {candidate}");
                if candidate == FALLBACK_SHORTCUT {
                    return Ok(FallbackOutcome::Switched(candidate));
                }
                if candidate == current.trim() {
                    return Ok(FallbackOutcome::RestoredOld(candidate));
                }
                return Ok(FallbackOutcome::Switched(candidate));
            }
            Err(error) => {
                tracing::warn!("热键 {candidate} 注册失败：{error}");
                last_error = error;
            }
        }
    }
    tracing::error!("所有热键候选都注册失败：{last_error}");
    Ok(FallbackOutcome::NoneRegistered)
}

/// 失焦即收走。
///
/// 召唤式弹窗失去焦点就该消失，否则它会一直压在人家的窗口上面。
/// 两个坑在这里被挡住：
/// - 刚唤出就收到的失焦事件（前台锁定）→ 宽限期忽略；
/// - 用户点托盘菜单导致失焦 → 只在窗口**确实可见**时才隐藏，且托盘菜单本身
///   还有"显示 Slip"，用户不会被锁在门外。
fn install_auto_dismiss(app: &App) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    let handle = app.handle().clone();
    window.on_window_event(move |event| {
        match event {
            WindowEvent::CloseRequested { api, .. } => {
                // 召唤式常驻托盘：关窗只是收起，不是退出。Alt+F4 / WM 关闭若销毁
                // 最后一个窗口，Tauri 会默认结束进程——托盘消失、dispatcher 的
                // shutdown 从未执行，正在写盘的 capture 被静默丢弃。
                // 真正的退出在托盘「退出」，它会先排空 dispatcher。
                api.prevent_close();
                if let Some(window) = handle.get_webview_window("main") {
                    let _ = window.hide();
                }
                FOCUSED_SINCE_SUMMON.store(false, Ordering::SeqCst);
                return;
            }
            WindowEvent::Focused(true) => {
                // 记下"这次唤出后确实被用过"，此后失焦才算真的离开。
                FOCUSED_SINCE_SUMMON.store(true, Ordering::SeqCst);
                return;
            }
            WindowEvent::Focused(false) => {}
            _ => return,
        }
        // 三道闸门（未上膛 / 从未聚焦 / 仍在宽限期）都在 `should_dismiss` 里，
        // 那边有单测；这里只负责把静态变量与时钟读成参数。
        let elapsed = LAST_SUMMON
            .lock()
            .ok()
            .and_then(|slot| *slot)
            .map(|at| at.elapsed());
        if !slip_app::focus::should_dismiss(
            AUTO_DISMISS_ARMED.load(Ordering::SeqCst),
            FOCUSED_SINCE_SUMMON.load(Ordering::SeqCst),
            elapsed,
            SUMMON_GRACE,
        ) {
            return;
        }
        if let Some(window) = handle.get_webview_window("main")
            && window.is_visible().unwrap_or(false)
        {
            let _ = window.hide();
            // 与 `dismiss` 保持一致：隐藏之后重新开始计"是否被聚焦过"。
            FOCUSED_SINCE_SUMMON.store(false, Ordering::SeqCst);
        }
    });
}

fn toggle(app: &AppHandle) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    let visible = window.is_visible().unwrap_or(false);
    let focused = window.is_focused().unwrap_or(false);
    if visible && focused {
        dismiss(app);
    } else {
        summon(app);
    }
}

/// 唤出：摆到光标旁边 → 显示 → 置顶 → 聚焦 → 通知前端。
///
/// 热键与托盘走这里：这是"我喊它出来"，所以**上膛**失焦收走。
pub fn summon(app: &AppHandle) {
    show(app, true);
}

/// 手动启动时的展示。
///
/// 与 [`summon`] 的唯一区别是**不上膛**失焦收走：用户明确打开了这个程序，
/// 别的程序抢前台不该让窗口消失。关闭它用 `Esc`、托盘或热键。
pub fn show_on_launch(app: &AppHandle) {
    show(app, false);
}

fn show(app: &AppHandle, arm_auto_dismiss: bool) {
    // 拿不到窗口时绝不能再静默返回：那会让"启动后没有界面"变成一条查不到原因的
    // 症状。这里必须留下日志，否则只能靠外部枚举窗口去猜。
    let Some(window) = app.get_webview_window("main") else {
        tracing::error!("main window is missing; show/summon skipped (UI will not appear)");
        return;
    };
    AUTO_DISMISS_ARMED.store(arm_auto_dismiss, Ordering::SeqCst);
    // 每一次显示都从"本次还没被聚焦过"重新开始。
    //
    // 为什么放在显示路径而不是每个隐藏路径里：`dismiss` 与失焦收走都会复位它，
    // 但隐藏的入口将来只会变多（关闭按钮转隐藏、托盘隐藏、系统最小化到托盘）。
    // 只要漏一个，下一次唤出就带着上一轮遗留的 `true`：一旦 `set_focus()` 失败
    // （Windows 前台锁定），400ms 后窗口会被自动收走，表现成"一闪就没"，
    // 而 `focus` 的单测全绿——纯函数分不清"本次唤出的聚焦"与"上次遗留的"。
    FOCUSED_SINCE_SUMMON.store(false, Ordering::SeqCst);
    // 先定位再显示：先显示会让人看见窗口从上一处跳过来。
    place_near_cursor(app, &window);
    let _ = window.show();
    let _ = window.set_always_on_top(true);
    let _ = window.set_focus();
    // 若窗口本来就处于聚焦态，`set_focus()` 不会再产生 `Focused(true)` 事件；
    // 直接以真实状态兜底，否则这个标志会一直停在 false，失焦就永不收走。
    if window.is_focused().unwrap_or(false) {
        FOCUSED_SINCE_SUMMON.store(true, Ordering::SeqCst);
    }
    note_summon();
    let _ = app.emit(SUMMON_EVENT, ());
}

/// 收走：隐藏窗口，焦点交还窗口系统。
pub fn dismiss(app: &AppHandle) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    let _ = window.hide();
    // 收走后重新开始计"是否被聚焦过"：下次唤出若始终没拿到焦点，
    // 就不该被失焦规则收走（否则双击启动会闪一下就没）。
    FOCUSED_SINCE_SUMMON.store(false, Ordering::SeqCst);
}

/// 把窗口摆到光标右下方；放不下就翻到另一侧，最后夹紧进工作区。
///
/// 任何一步拿不到信息都退化为居中——**宁可回到旧行为，也不能让窗口跑丢**。
fn place_near_cursor(app: &AppHandle, window: &tauri::WebviewWindow) {
    let Ok(cursor) = app.cursor_position() else {
        let _ = window.center();
        return;
    };
    let monitor = match app.monitor_from_point(cursor.x, cursor.y) {
        Ok(Some(monitor)) => monitor,
        _ => {
            let _ = window.center();
            return;
        }
    };
    let Ok(size) = window.outer_size() else {
        let _ = window.center();
        return;
    };

    // 用工作区而不是整屏：它已经扣掉了任务栏，窗口不会压在任务栏底下。
    let area = monitor.work_area();
    let left = area.position.x;
    let top = area.position.y;
    let bounds = Area {
        left,
        top,
        right: left + area.size.width as i32,
        bottom: top + area.size.height as i32,
    };

    // 物理尺寸对比的是物理坐标，所以缝也要按缩放折算。
    // R112：用的是**光标所在那块屏**的 `scale_factor()`（`monitor_from_point` 的返回值），
    // 不是主屏——混合 DPI 下拿主屏的倍率折算，副屏上 16 逻辑像素会差出一截。
    let gap = (CURSOR_GAP * monitor.scale_factor()).round() as i32;
    let (x, y) = place(
        (cursor.x.round() as i32, cursor.y.round() as i32),
        bounds,
        (size.width as i32, size.height as i32),
        gap,
    );
    // R113：`place` 只保证窗口留在工作区里（可以贴边）。贴边在 150% 缩放下看起来
    // 像"窗口被屏幕吃掉一个角"，所以再往里收 12 物理像素。
    // 这是**与工作区边缘**的关系，与 `CURSOR_GAP`（光标与窗口的缝）是两回事，别混。
    let (x, y) = clamp_with_work_area_margin(
        bounds,
        (size.width as i32, size.height as i32),
        (x, y),
        WORK_AREA_MARGIN,
    );
    let _ = window.set_position(PhysicalPosition::new(x, y));
}

/// 窗口与工作区边缘之间至少留出的距离（**物理像素**，R113）。
const WORK_AREA_MARGIN: i32 = 12;

/// 把窗口位置再夹进"工作区内缩 `margin`"的范围里。
///
/// 窗口比工作区还大时**绝不**继续留边：那会把左上角推到屏幕外，用户直接看不到标题。
/// 这种情况下退化为贴左上（`low`），把可见的部分尽量留在屏内。
fn clamp_with_work_area_margin(
    bounds: Area,
    size: (i32, i32),
    pos: (i32, i32),
    margin: i32,
) -> (i32, i32) {
    let clamp_axis = |value: i32, low: i32, high: i32| {
        if high < low {
            low
        } else {
            value.clamp(low, high)
        }
    };
    let (width, height) = size;
    let (x, y) = pos;
    (
        clamp_axis(x, bounds.left + margin, bounds.right - margin - width),
        clamp_axis(y, bounds.top + margin, bounds.bottom - margin - height),
    )
}

/// IPC：前端在 Esc 或捕获成功时请求收走。
#[tauri::command]
pub fn dismiss_window(app: AppHandle) {
    dismiss(&app);
}

#[cfg(test)]
mod tests {
    use super::{WORK_AREA_MARGIN, clamp_with_work_area_margin};
    use slip_app::geometry::Area;

    /// 1000×800 的工作区，原点在 (0,0)。
    fn area() -> Area {
        Area {
            left: 0,
            top: 0,
            right: 1000,
            bottom: 800,
        }
    }

    /// 本来就在安全区内：一个像素都不该动。
    #[test]
    fn an_already_inside_position_is_untouched() {
        let moved = clamp_with_work_area_margin(area(), (400, 300), (200, 150), WORK_AREA_MARGIN);
        assert_eq!(moved, (200, 150));
    }

    /// 贴边（`place` 允许）要被推进来 12px：右下角同样算。
    #[test]
    fn positions_on_the_edge_are_pushed_inward_by_the_margin() {
        let left = clamp_with_work_area_margin(area(), (400, 300), (-40, -40), WORK_AREA_MARGIN);
        assert_eq!(left, (WORK_AREA_MARGIN, WORK_AREA_MARGIN));

        let right = clamp_with_work_area_margin(area(), (400, 300), (990, 790), WORK_AREA_MARGIN);
        // 1000 - 12 - 400 = 588；800 - 12 - 300 = 488
        assert_eq!(right, (588, 488));
    }

    /// 窗口比工作区还大时**不能**继续留边，否则左上角会被推出屏幕。
    #[test]
    fn an_oversized_window_falls_back_to_the_inner_corner() {
        let moved = clamp_with_work_area_margin(area(), (1200, 900), (500, 400), WORK_AREA_MARGIN);
        assert_eq!(moved, (WORK_AREA_MARGIN, WORK_AREA_MARGIN));
    }

    // -------------------------------------------------- R116 回落三态到热键文本

    use super::{FallbackOutcome, fallback_candidates, next_hotkey};

    /// 候选顺序：备选 → 旧键 → 默认。空旧键时也要尝试默认。
    #[test]
    fn fallback_candidates_try_fallback_then_old_then_default() {
        assert_eq!(
            fallback_candidates("ctrl+alt+space"),
            vec![String::from("super+alt+s"), String::from("ctrl+alt+space")],
            "旧键就是默认键时不该重复"
        );
        assert_eq!(
            fallback_candidates(""),
            vec![String::from("super+alt+s"), String::from("ctrl+alt+space")],
            "没有旧键时仍要尝试默认键，否则重启后可能永无热键"
        );
        assert_eq!(
            fallback_candidates("super+alt+s"),
            vec![String::from("super+alt+s"), String::from("ctrl+alt+space")],
            "旧键等于备选键时不要重复"
        );
    }

    /// 成功换键：状态里必须写成**新键**，否则托盘继续显示旧键。
    #[test]
    fn switched_outcome_writes_the_new_key() {
        let outcome = FallbackOutcome::Switched(String::from("super+alt+s"));
        assert_eq!(
            next_hotkey(&outcome, "ctrl+alt+space"),
            Some(String::from("super+alt+s"))
        );
    }

    /// 备选失败但旧键恢复：状态保持为旧键，界面不该说"没热键"。
    #[test]
    fn restored_outcome_keeps_the_old_key() {
        let outcome = FallbackOutcome::RestoredOld(String::from("ctrl+alt+space"));
        assert_eq!(
            next_hotkey(&outcome, "已被改过的旧值"),
            Some(String::from("已被改过的旧值")),
            "以当前状态为准：恢复的是它自己，不是 outcome 里抄来的副本"
        );
    }

    /// 两个键都没注册上：状态必须置空，让 tooltip 与 `hotkey_status` 如实显示未注册。
    ///
    /// 这条是"界面不再骗人"的核心判定；若这里改回 `Some(current)`，
    /// 界面会继续展示一个按不响的热键。
    #[test]
    fn none_registered_outcome_clears_the_key() {
        assert_eq!(
            next_hotkey(&FallbackOutcome::NoneRegistered, "ctrl+alt+space"),
            None
        );
    }

    /// 负坐标副屏（副屏在主屏左侧时很常见）同样要留边。
    #[test]
    fn negative_origin_monitors_are_handled() {
        let second = Area {
            left: -1920,
            top: 0,
            right: 0,
            bottom: 1080,
        };
        let moved = clamp_with_work_area_margin(second, (400, 300), (-1920, 780), WORK_AREA_MARGIN);
        assert_eq!(moved, (-1908, 768));
    }
}
