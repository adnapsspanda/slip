//! 启动自检：把窗口与显示器的**实际**生效状态记进日志。
//!
//! 这里**不注册任何 IPC 命令**。窗口调研期的 `spike_*` 命令集已经删掉——它们
//! 不属于产品契约，而一个能被前端任意调用的 `set_ignore_cursor_events` 只会让
//! "点了窗口没反应" 变成可复现的用户故障。
//!
//! 平时不输出，排查时用 `SLIP_LOG=debug` 启动即可看到。判据：句柄在、但外部
//! `EnumWindows` 枚举不到 = 窗口已销毁，或建在了另一个窗口站/桌面上。

use tauri::WebviewWindow;

/// 记一条窗口状态。字段直接写进日志，不经过中间结构体——
/// 那些结构体只会被 `Debug` 打印，字段等于从未被读取。
pub fn log_window(window: &WebviewWindow) {
    let Ok(position) = window.outer_position() else {
        tracing::warn!("window report unavailable: outer_position failed");
        return;
    };
    let Ok(size) = window.outer_size() else {
        tracing::warn!("window report unavailable: outer_size failed");
        return;
    };
    tracing::debug!(
        label = window.label(),
        hwnd = window.hwnd().map(|h| h.0 as isize).unwrap_or(0),
        visible = window.is_visible().unwrap_or(false),
        focused = window.is_focused().unwrap_or(false),
        always_on_top = window.is_always_on_top().unwrap_or(false),
        scale = window.scale_factor().unwrap_or(0.0),
        x = position.x,
        y = position.y,
        width = size.width,
        height = size.height,
        "window report"
    );
}

/// 记每个显示器的位置、尺寸与缩放。多屏/混合 DPI 的摆放问题全靠这一段定位。
pub fn log_monitors(window: &WebviewWindow) {
    let primary = window
        .primary_monitor()
        .ok()
        .flatten()
        .map(|monitor| *monitor.position());
    let Ok(monitors) = window.available_monitors() else {
        tracing::warn!("monitor list unavailable");
        return;
    };
    for monitor in monitors {
        let position = *monitor.position();
        tracing::debug!(
            name = monitor.name().map(String::as_str).unwrap_or("<unnamed>"),
            x = position.x,
            y = position.y,
            width = monitor.size().width,
            height = monitor.size().height,
            scale = monitor.scale_factor(),
            primary = primary.is_some_and(|p| p == position),
            "monitor"
        );
    }
}
