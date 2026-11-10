//! §9.7 窗口 spike 的最小命令集。
//!
//! 这些命令**不属于产品契约**，只用来在没有 devtools 的情况下取证。
//! 窗口 spike 结论确定后，本模块应当整体删除。

use serde::Serialize;
use tauri::WebviewWindow;

#[derive(Debug, Serialize)]
pub struct MonitorInfo {
    pub name: String,
    pub width: u32,
    pub height: u32,
    /// 缩放比例，1.0 = 100%。
    pub scale: f64,
    pub x: i32,
    pub y: i32,
    pub primary: bool,
}

#[derive(Debug, Serialize)]
pub struct WindowReport {
    pub label: String,
    /// 窗口当前所在显示器的 DPI 缩放。
    pub scale_factor: f64,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub always_on_top: bool,
    pub focused: bool,
}

/// 点击穿透开关（§9.7 第 1 条）。
///
/// `true` 时整个窗口不再接收鼠标事件，事件落到下层应用。
#[tauri::command]
pub fn spike_ignore_cursor(window: WebviewWindow, ignore: bool) -> Result<(), String> {
    window
        .set_ignore_cursor_events(ignore)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn spike_always_on_top(window: WebviewWindow, on: bool) -> Result<(), String> {
    window
        .set_always_on_top(on)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn spike_monitors(window: WebviewWindow) -> Result<Vec<MonitorInfo>, String> {
    let primary = window
        .primary_monitor()
        .map_err(|error| error.to_string())?
        .map(|monitor| *monitor.position());
    let monitors = window
        .available_monitors()
        .map_err(|error| error.to_string())?;
    Ok(monitors
        .into_iter()
        .map(|monitor| {
            let position = monitor.position();
            MonitorInfo {
                name: monitor
                    .name()
                    .cloned()
                    .unwrap_or_else(|| String::from("<unnamed>")),
                width: monitor.size().width,
                height: monitor.size().height,
                scale: monitor.scale_factor(),
                x: position.x,
                y: position.y,
                primary: primary.is_some_and(|p| p == *position),
            }
        })
        .collect())
}

/// 读回窗口**实际**生效的属性。配置里写了不等于平台照做了。
#[tauri::command]
pub fn spike_window_report(window: WebviewWindow) -> Result<WindowReport, String> {
    let position = window.outer_position().map_err(|e| e.to_string())?;
    let size = window.outer_size().map_err(|e| e.to_string())?;
    let scale_factor = window.scale_factor().map_err(|e| e.to_string())?;
    let always_on_top = window.is_always_on_top().map_err(|e| e.to_string())?;
    let focused = window.is_focused().map_err(|e| e.to_string())?;
    Ok(WindowReport {
        label: window.label().to_owned(),
        scale_factor,
        x: position.x,
        y: position.y,
        width: size.width,
        height: size.height,
        always_on_top,
        focused,
    })
}

/// §9.7 第 4 条：捕获框关闭后焦点是否回到原应用。
#[tauri::command]
pub fn spike_hide(window: WebviewWindow) -> Result<(), String> {
    window.hide().map_err(|error| error.to_string())
}

#[tauri::command]
pub fn spike_show(window: WebviewWindow) -> Result<(), String> {
    window.show().map_err(|error| error.to_string())?;
    window.set_focus().map_err(|error| error.to_string())
}
