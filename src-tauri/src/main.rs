// 发布构建不弹控制台窗口；调试构建保留 stdout，方便读窗口报告。
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

fn main() {
    slip_shell::run();
}
