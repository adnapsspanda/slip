//! 托盘图标、开机自启与"打开 Inbox"入口。
//!
//! 这里没有业务判断，只有四个入口：显示窗口、用系统默认程序打开 `inbox.md`、
//! 切换开机自启、退出。开机自启的**真源是操作系统**：菜单勾选只是镜像，
//! 每次切换都以 `is_enabled()` 的读数为准，失败时把勾选拉回真实状态。
//!
//! 托盘还是"召唤式"形态下的**唯一入口**：窗口默认隐藏，用户只能靠这个图标
//! 知道 Slip 在跑、以及怎么把它叫出来。所以 tooltip 与菜单里的热键必须来自
//! 托管状态里那个**真正注册成功**的字符串，不能写死常量——否则用户改了配置
//! 后，界面还在展示旧热键，等于骗人。

use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{App, AppHandle, Manager};
use tauri_plugin_autostart::ManagerExt;

/// 退出时把非阻塞的收尾放到这里。
/// 托盘里"待办摘要"那一行的句柄。
///
/// 为什么不重建整个菜单：菜单重建会打断用户正在展开的菜单，而且每次变更都重建是浪费。
/// 只 `set_text` 一行就够，且**只有计数变化时才调用**。
pub struct TraySummary(pub MenuItem<tauri::Wry>);

/// 读取当前未完成条数。拿不到快照时按 0 处理（托盘不该因为读状态失败而消失）。
fn current_pending(app: &App) -> usize {
    let handle = app.handle().clone();
    let Some(state) = handle.try_state::<crate::ShellState>() else {
        return 0;
    };
    match tauri::async_runtime::block_on(state.dispatcher.get_snapshot()) {
        Ok(snapshot) => snapshot.slips.iter().filter(|slip| !slip.done).count(),
        Err(_) => 0,
    }
}

/// 把托盘的摘要行与 tooltip 更新到最新计数。
///
/// 幂等：调用方只需在**计数变化时**调它。
pub fn refresh_tray(app: &tauri::AppHandle, pending: usize) {
    // 留一条日志：托盘是"看不见的状态"，出问题时没有痕迹就只能靠猜。
    tracing::info!("tray summary: {}", slip_app::tray::summary(pending));
    if let Some(summary) = app.try_state::<TraySummary>() {
        let _ = summary.0.set_text(slip_app::tray::summary(pending));
    }
    if let Some(tray) = app.tray_by_id("slip") {
        let pretty = app
            .try_state::<crate::ShellState>()
            .map(|state| pretty_hotkey(&state.hotkey))
            .unwrap_or_default();
        let _ = tray.set_tooltip(Some(slip_app::tray::tooltip(&pretty, pending)));
    }
}

pub fn install_tray(app: &App) -> Result<(), Box<dyn std::error::Error>> {
    // 热键从托管状态读：`hotkey` 已经过解析校验，解析失败时也已回落默认值，
    // 所以这里拿到的就是**此刻真正生效**的那一组键。
    let hotkey = app.state::<crate::ShellState>().hotkey.clone();
    let pretty = pretty_hotkey(&hotkey);
    let hotkey_text = if hotkey.is_empty() {
        String::from("热键：未注册")
    } else {
        format!("热键：{pretty}")
    };

    // 置灰的信息项放在最上面：用户第一眼看到的就是"这东西怎么叫出来"。
    // enabled = false 让它不可点击，纯粹当一行说明用（回调里因此不需要分支）。
    let hotkey_item = MenuItem::with_id(app, "hotkey", hotkey_text, false, None::<&str>)?;
    // 待办摘要：同样是**置灰的信息项**，随快照更新（见 `refresh_tray`）。
    // 初值取此刻的实时快照——启动时任务栏/托盘就该是准确的，不能等第一次变更。
    let pending_now = current_pending(app);
    // tooltip 与菜单摘要用同一份计数：悬停气泡和菜单不能各说各话。
    let tooltip = slip_app::tray::tooltip(
        if hotkey.is_empty() {
            ""
        } else {
            pretty.as_str()
        },
        pending_now,
    );
    let summary_item = MenuItem::with_id(
        app,
        "summary",
        slip_app::tray::summary(pending_now),
        false,
        None::<&str>,
    )?;
    let summary_for_state = summary_item.clone();
    let separator = PredefinedMenuItem::separator(app)?;
    let show = MenuItem::with_id(app, "show", "显示 Slip", true, None::<&str>)?;
    let open = MenuItem::with_id(app, "open", "打开 Inbox", true, None::<&str>)?;
    // 初始勾选读系统状态，而不是写死 false：用户可能在上次运行后手动改过注册表。
    let autostart = CheckMenuItem::with_id(
        app,
        "autostart",
        "开机自启",
        true,
        autostart_enabled(app),
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &hotkey_item,
            &summary_item,
            &separator,
            &show,
            &open,
            &autostart,
            &quit,
        ],
    )?;

    // 菜单事件回调里要改勾选状态，所以把句柄搬进闭包（内部是 Arc，克隆很便宜）。
    let autostart_item = autostart.clone();

    let icon = app
        .default_window_icon()
        .cloned()
        .ok_or("no default window icon; set one in tauri.conf.json")?;

    TrayIconBuilder::with_id("slip")
        .icon(icon)
        .tooltip(&tooltip)
        .menu(&menu)
        // 左键已经绑定"唤出"，不能再由它弹菜单：窗口拿到焦点会立刻把菜单关掉，
        // 用户只会看到菜单闪一下，拿不到"打开 Inbox / 开机自启 / 退出"。
        // 于是左键 = 唤出，右键 = 菜单——托盘应用的通行分工。
        .show_menu_on_left_click(false)
        .on_menu_event(move |app, event| match event.id().as_ref() {
            "show" => {
                crate::hotkey::summon(app);
            }
            "open" => {
                let _ = open_inbox(app);
            }
            "autostart" => toggle_autostart(app, &autostart_item),
            "quit" => {
                let handle = app.clone();
                tauri::async_runtime::spawn(async move {
                    if let Some(state) = handle.try_state::<crate::ShellState>() {
                        state.dispatcher.shutdown().await;
                    }
                    handle.exit(0);
                });
            }
            // `hotkey` 是 enabled = false 的信息项，Windows 上不会投递事件；
            // 其余 id（含分隔符）一律忽略。
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                crate::hotkey::summon(tray.app_handle());
            }
        })
        .build(app)?;

    // 有这条日志，`tray install failed` 的缺席才算有意义。
    tracing::info!("tray installed: {tooltip}");
    // 句柄托管必须在返回之前：刷新任务要靠它拿到那一行去 set_text。
    app.manage(TraySummary(summary_for_state));
    Ok(())
}
/// 把小写的热键文本美化成给人看的形式：`ctrl+alt+space` → `Ctrl+Alt+Space`。
///
/// 托管状态里的值统一是小写（见 `hotkey::resolve_shortcut`），直接塞进菜单
/// 和 tooltip 会显得粗糙；按 `+` 分段、每段首字母大写即可，不做键名语义映射
/// ——`Space`、`F1`、`PageUp` 这类名字的具体写法由用户自己保持原样更稳妥。
pub(crate) fn pretty_hotkey(raw: &str) -> String {
    raw.split('+')
        .map(|part| {
            let part = part.trim();
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join("+")
}

/// 读系统里的真实自启状态；插件没装或读取失败时都退回"未启用"，不阻断托盘构建。
fn autostart_enabled(app: &App) -> bool {
    // `autolaunch()` 走 `State` 取插件托管状态，插件缺失会 panic；先探一下再取。
    if app
        .try_state::<tauri_plugin_autostart::AutoLaunchManager>()
        .is_none()
    {
        tracing::error!("autostart plugin state missing; treat autostart as disabled");
        return false;
    }
    match app.autolaunch().is_enabled() {
        Ok(checked) => checked,
        Err(error) => {
            tracing::error!("autostart: cannot read current state: {error}");
            false
        }
    }
}

/// 切换开机自启，并把菜单勾选同步回系统的真实状态。
fn toggle_autostart(app: &AppHandle, item: &CheckMenuItem<tauri::Wry>) {
    let current = match app.autolaunch().is_enabled() {
        Ok(current) => current,
        Err(error) => {
            tracing::error!("autostart: cannot read current state: {error}");
            sync_checked(app, item);
            return;
        }
    };

    let target = !current;
    let result = if target {
        app.autolaunch().enable()
    } else {
        app.autolaunch().disable()
    };

    match result {
        Ok(()) => {
            if let Err(error) = item.set_checked(target) {
                tracing::error!("autostart: cannot update menu check state: {error}");
            }
            tracing::info!("autostart {}", if target { "enabled" } else { "disabled" });
        }
        Err(error) => {
            // 失败不改勾选：界面上必须显示"实际没生效"。
            tracing::error!("autostart: cannot set enabled={target}: {error}");
            sync_checked(app, item);
        }
    }
}

/// 用系统读数覆盖菜单勾选；连读数都拿不到就保持原样。
fn sync_checked(app: &AppHandle, item: &CheckMenuItem<tauri::Wry>) {
    match app.autolaunch().is_enabled() {
        Ok(checked) => {
            if let Err(error) = item.set_checked(checked) {
                tracing::error!("autostart: cannot sync menu check state: {error}");
            }
        }
        Err(error) => tracing::error!("autostart: cannot read state to sync menu: {error}"),
    }
}

/// 用系统默认程序打开 inbox.md；打不开时退而打开 vault 目录。
fn open_inbox(app: &AppHandle) -> Result<(), String> {
    let vault = app.state::<crate::ShellState>().vault.clone();
    let inbox = vault.join("inbox.md");
    let target = if inbox.exists() { inbox } else { vault };
    // 只 spawn、不 wait：菜单回调跑在 UI 线程上，等子进程退出会卡住整个托盘。
    if let Err(error) = std::process::Command::new("explorer.exe")
        .arg(&target)
        .spawn()
    {
        return Err(format!("无法打开 {target:?}: {error}"));
    }
    Ok(())
}

/// 链接**能不能开**的判定，与"怎么开"分开——这样才能单测而不真的弹浏览器。
pub fn validate_url(url: &str) -> Result<(), String> {
    let http = url.starts_with("http://") && url.len() > "http://".len();
    let https = url.starts_with("https://") && url.len() > "https://".len();
    if !http && !https {
        return Err(String::from("只允许 http/https 链接"));
    }
    if url.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(String::from("链接里不能有空白或控制字符"));
    }
    Ok(())
}

/// 用系统默认浏览器打开一个 http/https 链接。
///
/// 三条边界：
/// - **只认 http/https**：别的前缀（`file:`、`javascript:`、`ms-settings:`…）一律拒绝，
///   否则正文里写一个 `file:///C:/Windows/System32/...` 就等于给了个启动器；
/// - 含空白或控制字符的直接拒——URL 里合法字符很多，但空白和控制字符不该出现在这里；
/// - 用 `rundll32 url.dll,FileProtocolHandler`，**URL 作为单个 argv 传递**，
///   不经过 `cmd /c start`：后者会把 `&` 当命令分隔符，那是注入。
pub fn open_url(url: &str) -> Result<(), String> {
    validate_url(url)?;
    if let Err(error) = std::process::Command::new("rundll32.exe")
        .arg("url.dll,FileProtocolHandler")
        .arg(url)
        .spawn()
    {
        return Err(format!("无法打开链接：{error}"));
    }
    Ok(())
}

/// vault 里遗留的隔离/损坏证据文件（`inbox.md.corrupt-*`），新的在前。
///
/// 只认这个前缀：vault 是用户可写的目录，里面可能有他自己放的备份或笔记，
/// 扫出别的文件并显示成"损坏"会误导人。
pub fn corrupt_artifacts(vault: &std::path::Path) -> Vec<std::path::PathBuf> {
    let Ok(entries) = std::fs::read_dir(vault) else {
        return Vec::new();
    };
    let mut found: Vec<std::path::PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("inbox.md.corrupt-"))
        })
        .collect();
    // 新的在前。取不到 mtime 时按文件名倒序——时间戳就在名字里。
    found.sort_by(|left, right| {
        let ml = std::fs::metadata(left).and_then(|m| m.modified()).ok();
        let mr = std::fs::metadata(right).and_then(|m| m.modified()).ok();
        mr.cmp(&ml).then_with(|| right.cmp(left))
    });
    found
}

/// 外部修改的差异统计。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize)]
pub struct DiffSummary {
    /// 磁盘上有、内存里没有的条数（外部新增）。
    pub added: usize,
    /// 内存里有、磁盘上没了的条数（外部删除）。
    pub removed: usize,
    /// 两边都有但正文或完成态变了的条数。
    pub changed: usize,
}

/// 比较"界面现在显示的"与"磁盘现在的"，给出条数级差异。
///
/// 用 `(id, text, done)` 三元组做键：只关心**人会注意到的变化**，
/// 时间戳或标记顺序的差异不算改动。
pub fn diff_summary(
    current: &[(String, String, bool)],
    disk: &[(String, String, bool)],
) -> DiffSummary {
    use std::collections::BTreeMap;

    let current_map: BTreeMap<&str, (&str, bool)> = current
        .iter()
        .map(|(id, text, done)| (id.as_str(), (text.as_str(), *done)))
        .collect();
    let disk_map: BTreeMap<&str, (&str, bool)> = disk
        .iter()
        .map(|(id, text, done)| (id.as_str(), (text.as_str(), *done)))
        .collect();

    let mut summary = DiffSummary::default();
    for (id, value) in &disk_map {
        match current_map.get(id) {
            None => summary.added += 1,
            Some(old) if old != value => summary.changed += 1,
            Some(_) => {}
        }
    }
    for id in current_map.keys() {
        if !disk_map.contains_key(id) {
            summary.removed += 1;
        }
    }
    summary
}

/// 把当前清单渲染成人能读的 Markdown 清单（**不含** `<!-- slip:v1 -->` 标记）。
///
/// 为什么单独渲染而不是复制 `inbox.md`：后者带着机器标记，粘到别处既脏又可能被别的
/// 工具误解析。这里只输出 `- [ ]` / `- [x]` 与正文。
///
/// 纯函数，不碰磁盘——导出是"给人看"的，不该因为写文件失败就无法测试。
pub fn render_plain(title: &str, slips: &[(&str, bool)]) -> String {
    let mut out = format!("# {title}\n\n");
    if slips.is_empty() {
        out.push_str("（没有内容）\n");
        return out;
    }
    for (text, done) in slips {
        let mark = if *done { "x" } else { " " };
        out.push_str(&format!("- [{mark}] {text}\n"));
    }
    // 统一成本平台原生换行：Windows 记事本不认纯 LF，直接打开会连成一片。
    // 不加 BOM——那会让第一行的标题解析失败。
    slip_domain::normalize_newlines(&out)
}

/// 把导出内容写到 vault 下的 `export-<unix_ms>.md`，返回路径。
pub fn write_export(vault: &std::path::Path, content: &str) -> Result<std::path::PathBuf, String> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let path = vault.join(format!("export-{stamp}.md"));
    if let Err(error) = std::fs::write(&path, content) {
        return Err(format!("写入失败：{error}"));
    }
    Ok(path)
}

/// 在资源管理器里选中最新那个损坏文件。
///
/// **不接收前端传来的路径**：那等于让 webview 决定 explorer 打开什么。
/// 需要什么由 Rust 自己从 vault 里查。
pub fn reveal_newest_corrupt(app: &AppHandle) -> Result<(), String> {
    let vault = app.state::<crate::ShellState>().vault.clone();
    let Some(newest) = corrupt_artifacts(&vault).into_iter().next() else {
        return Ok(());
    };
    // `/select,<path>` 必须是**一个**参数，否则 explorer 会把路径当成要独立打开的目标。
    let arg = format!("/select,{}", newest.display());
    if let Err(error) = std::process::Command::new("explorer.exe").arg(arg).spawn() {
        return Err(format!("无法定位 {}: {error}", newest.display()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{diff_summary, render_plain, validate_url};

    fn row(id: &str, text: &str, done: bool) -> (String, String, bool) {
        (id.to_owned(), text.to_owned(), done)
    }

    #[test]
    fn diff_counts_added_removed_and_changed() {
        let current = vec![
            row("a", "留下", false),
            row("b", "被改过", false),
            row("c", "被删了", false),
        ];
        let disk = vec![
            row("a", "留下", false),
            row("b", "改过了", false),
            row("d", "外部新增", true),
        ];
        let summary = diff_summary(&current, &disk);
        assert_eq!(summary.added, 1, "d 是外部新增");
        assert_eq!(summary.removed, 1, "c 在磁盘上没了");
        assert_eq!(summary.changed, 1, "b 的正文变了");
    }

    #[test]
    fn diff_ignores_identical_state() {
        // 内容没变就不该报差异——否则每次外部碰一下文件都会弹提示。
        let rows = vec![row("a", "一样", false), row("b", "也一样", true)];
        let summary = diff_summary(&rows, &rows);
        assert_eq!(summary, super::DiffSummary::default());
    }

    #[test]
    fn done_flag_change_counts_as_changed() {
        let current = vec![row("a", "同一条", false)];
        let disk = vec![row("a", "同一条", true)];
        assert_eq!(diff_summary(&current, &disk).changed, 1);
    }

    #[test]
    fn render_plain_marks_state_and_drops_markers() {
        // 导出的东西是给人看的：不能带 <!-- slip:v1 --> 这种人造标记。
        let out = render_plain("Slip 导出", &[("买发票", false), ("已完成的", true)]);
        // 换行是**平台原生**的（Windows 记事本不认纯 LF），所以断言要用同一个来源，
        // 不能写死 \n——否则这条测试在 Windows 上必红。
        let nl = slip_domain::native_newline();
        assert_eq!(
            out,
            format!("# Slip 导出{nl}{nl}- [ ] 买发票{nl}- [x] 已完成的{nl}")
        );
    }

    /// 导出文件在 Windows 上必须是 CRLF，且**不能带 BOM**。
    #[test]
    fn render_plain_uses_native_newlines_without_bom() {
        let out = render_plain("导出", &[("一条", false)]);
        if cfg!(windows) {
            assert!(out.contains("\r\n"), "Windows 上应当是 CRLF：{out:?}");
            assert!(!out.contains("\n\n"), "不应残留纯 LF 换行");
        } else {
            assert!(!out.contains('\r'), "非 Windows 上不应出现 CR：{out:?}");
        }
        assert!(!out.starts_with('\u{feff}'), "导出内容不应带 BOM");
    }

    #[test]
    fn render_plain_says_so_when_empty() {
        let out = render_plain("Slip 导出", &[]);
        assert!(out.contains("没有内容"), "{out}");
    }

    #[test]
    fn accepts_plain_http_and_https() {
        assert!(validate_url("http://example.com").is_ok());
        assert!(validate_url("https://example.com/a?b=c&d=e#f").is_ok());
    }

    #[test]
    fn rejects_other_schemes() {
        // 正文是用户可编辑的文件：任何非 http(s) 前缀都不该变成启动器。
        // 特别是 file: 与 javascript: ——前者能打开本地任意路径，后者能执行脚本。
        for url in [
            "file:///C:/Windows/System32/cmd.exe",
            "javascript:alert(1)",
            "ms-settings:privacy",
            "vbscript:msgbox(1)",
            "http://",
            "https://",
            "example.com",
        ] {
            assert!(validate_url(url).is_err(), "should reject {url}");
        }
    }

    #[test]
    fn rejects_whitespace_and_control_characters() {
        for url in [
            "https://example.com/a b",
            "https://example.com/a\tb",
            "https://example.com/a\nb",
            "https://example.com/a\u{0}b",
        ] {
            assert!(validate_url(url).is_err(), "should reject {url:?}");
        }
    }
}
