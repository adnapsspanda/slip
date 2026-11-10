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
pub fn install_tray(app: &App) -> Result<(), Box<dyn std::error::Error>> {
    // 热键从托管状态读：`hotkey` 已经过解析校验，解析失败时也已回落默认值，
    // 所以这里拿到的就是**此刻真正生效**的那一组键。
    let hotkey = app.state::<crate::ShellState>().hotkey.clone();
    let pretty = pretty_hotkey(&hotkey);
    let (tooltip, hotkey_text) = if hotkey.is_empty() {
        (
            String::from("Slip · 热键未注册"),
            String::from("热键：未注册"),
        )
    } else {
        (format!("Slip · {pretty} 唤出"), format!("热键：{pretty}"))
    };

    // 置灰的信息项放在最上面：用户第一眼看到的就是"这东西怎么叫出来"。
    // enabled = false 让它不可点击，纯粹当一行说明用（回调里因此不需要分支）。
    let hotkey_item = MenuItem::with_id(app, "hotkey", hotkey_text, false, None::<&str>)?;
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
        &[&hotkey_item, &separator, &show, &open, &autostart, &quit],
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
    Ok(())
}

/// 把小写的热键文本美化成给人看的形式：`ctrl+alt+space` → `Ctrl+Alt+Space`。
///
/// 托管状态里的值统一是小写（见 `hotkey::resolve_shortcut`），直接塞进菜单
/// 和 tooltip 会显得粗糙；按 `+` 分段、每段首字母大写即可，不做键名语义映射
/// ——`Space`、`F1`、`PageUp` 这类名字的具体写法由用户自己保持原样更稳妥。
fn pretty_hotkey(raw: &str) -> String {
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
