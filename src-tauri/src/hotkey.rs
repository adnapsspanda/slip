//! 全局快捷键与"唤出 / 收走"生命周期。
//!
//! 产品形态是**召唤式**：窗口默认隐藏，热键唤出，记完或 Esc 收走。
//! 本模块只处理"窗口怎么出现、出现在哪、什么时候消失"，不碰任何业务判断。

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use tauri::{App, AppHandle, Emitter, Manager, PhysicalPosition, WindowEvent};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

/// 默认热键。选 `Ctrl+Alt+Space`：不占 `Alt+Space`（系统菜单），
/// 也不与常见粘贴板 / 输入法快捷键冲突。
const DEFAULT_SHORTCUT: &str = "ctrl+alt+space";

/// 前端监听此事件来把光标放进捕获框并清掉提示。
const SUMMON_EVENT: &str = "slip://summon";

/// 唤出后多久之内收到的"失焦"事件要忽略。
///
/// 原因：Windows 的前台锁定可能让 `set_focus()` 失败，紧接着系统就送来一个
/// `Focused(false)`；不设宽限期就会变成"窗口一闪就没了"。
const SUMMON_GRACE: Duration = Duration::from_millis(400);

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

fn recently_summoned() -> bool {
    LAST_SUMMON
        .lock()
        .ok()
        .and_then(|slot| *slot)
        .is_some_and(|at| at.elapsed() < SUMMON_GRACE)
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
    app.global_shortcut()
        .on_shortcut(shortcut, |app, _shortcut, event| {
            // 只响应按下：否则一次按键会触发 Pressed + Released 两次。
            if event.state() == ShortcutState::Pressed {
                toggle(app);
            }
        })?;
    tracing::info!("hotkey registered: {text}");
    Ok(())
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
            WindowEvent::Focused(true) => {
                // 记下"这次唤出后确实被用过"，此后失焦才算真的离开。
                FOCUSED_SINCE_SUMMON.store(true, Ordering::SeqCst);
                return;
            }
            WindowEvent::Focused(false) => {}
            _ => return,
        }
        // 从未聚焦过 → 不是"失去焦点"，是"还没拿到焦点"，收走等于让窗口闪现即消失。
        if !AUTO_DISMISS_ARMED.load(Ordering::SeqCst) {
            return;
        }
        if !FOCUSED_SINCE_SUMMON.load(Ordering::SeqCst) || recently_summoned() {
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

/// 一个矩形区域（物理像素）。抽成独立类型是为了让几何计算能被单测。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Area {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

/// 纯几何：算出窗口左上角应该落在哪里。
///
/// 规则与 [`place_near_cursor`] 完全一致，只是把"读光标 / 读显示器 / 读尺寸"这些
/// 需要 `AppHandle` 的部分剥离出去——否则这段最容易出错的边界逻辑永远只能靠
/// 端到端脚本碰运气验证，而负坐标副屏那条在单屏机器上还是 SKIP。
///
/// 顺序不能改：先尝试右下（`cursor + gap`），放不下再翻到左上，最后夹紧进工作区。
/// 夹紧用 `.max()` 兜住"窗口比工作区还大"的退化情形——`clamp` 在 `min > max` 时会 panic。
fn placement(cursor: (i32, i32), area: Area, size: (i32, i32), gap: i32) -> (i32, i32) {
    let (cursor_x, cursor_y) = cursor;
    let (width, height) = size;

    let mut x = cursor_x + gap;
    if x + width > area.right {
        x = cursor_x - gap - width;
    }
    let mut y = cursor_y + gap;
    if y + height > area.bottom {
        y = cursor_y - gap - height;
    }

    let max_x = (area.right - width).max(area.left);
    let max_y = (area.bottom - height).max(area.top);
    (x.clamp(area.left, max_x), y.clamp(area.top, max_y))
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
    let gap = (CURSOR_GAP * monitor.scale_factor()).round() as i32;
    let (x, y) = placement(
        (cursor.x.round() as i32, cursor.y.round() as i32),
        bounds,
        (size.width as i32, size.height as i32),
        gap,
    );
    let _ = window.set_position(PhysicalPosition::new(x, y));
}

/// IPC：前端在 Esc 或捕获成功时请求收走。
#[tauri::command]
pub fn dismiss_window(app: AppHandle) {
    dismiss(&app);
}

#[cfg(test)]
mod tests {
    use super::{Area, placement};

    /// 主屏常见值：1707×1019 的虚拟工作区（2560×1600 @150% 扣掉任务栏）。
    fn primary() -> Area {
        Area {
            left: 0,
            top: 0,
            right: 1707,
            bottom: 1019,
        }
    }

    const WIN: (i32, i32) = (420, 560);
    const GAP: i32 = 16;

    #[test]
    fn right_below_when_it_fits() {
        // 左上角放得下：直接落在光标右下方，不留意外偏移。
        assert_eq!(placement((200, 200), primary(), WIN, GAP), (216, 216));
    }

    #[test]
    fn flips_left_when_it_would_overflow() {
        // 1400 + 16 + 420 = 1836 > 1707 → 翻到光标左侧。
        let (x, _) = placement((1400, 100), primary(), WIN, GAP);
        assert_eq!(x, 1400 - GAP - 420);
    }

    #[test]
    fn flips_up_when_it_would_overflow() {
        // 900 + 16 + 560 = 1476 > 1019 → 翻到光标上方。
        let (_, y) = placement((100, 900), primary(), WIN, GAP);
        assert_eq!(y, 900 - GAP - 560);
    }

    #[test]
    fn exactly_at_the_edge_does_not_flip() {
        // 判据是严格大于：正好贴边仍算放得下。
        let cursor_x = 1707 - 420 - GAP;
        let (x, _) = placement((cursor_x, 100), primary(), WIN, GAP);
        assert_eq!(x, cursor_x + GAP);

        // 再多 1 像素就必须翻。
        let (x, _) = placement((cursor_x + 1, 100), primary(), WIN, GAP);
        assert_eq!(x, cursor_x + 1 - GAP - 420);
    }

    #[test]
    fn handles_monitor_with_negative_origin() {
        // 副屏整体落在负坐标区（本机 DISPLAY5 就是这种布局）。
        // 这条路径端到端只能 SKIP（单屏时脚本会跳过副屏场景），所以必须在这里钉住。
        let secondary = Area {
            left: -454,
            top: -1440,
            right: 2106,
            bottom: -48,
        };
        let (x, y) = placement((-300, -1000), secondary, WIN, GAP);
        assert_eq!((x, y), (-284, -984));
        assert!(x >= secondary.left && x + WIN.0 <= secondary.right);
        assert!(y >= secondary.top && y + WIN.1 <= secondary.bottom);
    }

    #[test]
    fn clamps_when_window_is_larger_than_work_area() {
        // 退化情形：窗口比工作区还大。必须保住左上角可见，且**不能 panic**
        // （`clamp` 在 min > max 时会炸，靠 `.max()` 兜住）。
        let tiny = Area {
            left: 0,
            top: 0,
            right: 300,
            bottom: 200,
        };
        assert_eq!(placement((250, 150), tiny, WIN, GAP), (0, 0));
    }

    #[test]
    fn stays_inside_work_area_for_every_edge_cursor() {
        // 扫一遍边界：无论光标落在工作区的哪个角落，结果都必须完整落在工作区内。
        let cases = [
            (-1000, -1000),
            (0, 0),
            (1707, 0),
            (0, 1019),
            (1707, 1019),
            (850, 500),
        ];
        for (cx, cy) in cases {
            let (x, y) = placement((cx, cy), primary(), WIN, GAP);
            assert!(x >= 0, "cursor=({cx},{cy}) x={x} 越出左边");
            assert!(x + WIN.0 <= 1707, "cursor=({cx},{cy}) x={x} 越出右边");
            assert!(y >= 0, "cursor=({cx},{cy}) y={y} 越出上边");
            assert!(y + WIN.1 <= 1019, "cursor=({cx},{cy}) y={y} 越出下边");
        }
    }
}
