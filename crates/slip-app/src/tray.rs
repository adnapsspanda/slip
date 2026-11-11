//! 托盘文案。
//!
//! 抽成纯函数而不是直接写在 `install_tray` 里，理由和 `geometry` 一样：托盘在
//! `src-tauri`（依赖 tauri），那边的测试在本机时常因 Smart App Control 跑不起来；
//! 而"0 条待办要不要显示数量""单复数怎么写"这类判断，恰恰是最容易写歪、又最该被钉住的部分。

/// 托盘菜单里的待办摘要。
///
/// 规则：
/// - 有未完成条目 → `N 项待办`；
/// - 全部完成或本来就是空 → `暂无待办`（不写 `0 项待办`——那读起来像出了错）。
pub fn summary(pending: usize) -> String {
    if pending == 0 {
        String::from("暂无待办")
    } else {
        format!("{pending} 项待办")
    }
}

/// 托盘图标提示（鼠标悬停）。
///
/// 与菜单项共用同一份计数，但多带了产品名——悬停时用户看到的是一个独立气泡，
/// 只说"3 项待办"会不知道这是谁的。
pub fn tooltip(hotkey_pretty: &str, pending: usize) -> String {
    if hotkey_pretty.is_empty() {
        format!("Slip · {} · 热键未注册", summary(pending))
    } else {
        format!("Slip · {} · {hotkey_pretty} 唤出", summary(pending))
    }
}

#[cfg(test)]
mod tests {
    use super::{summary, tooltip};

    #[test]
    fn summary_counts_pending_items() {
        assert_eq!(summary(1), "1 项待办");
        assert_eq!(summary(3), "3 项待办");
        assert_eq!(summary(128), "128 项待办");
    }

    #[test]
    fn summary_says_none_instead_of_zero() {
        // "0 项待办"读起来像异常；空就是空。
        assert_eq!(summary(0), "暂无待办");
        assert!(!summary(0).contains('0'), "不该出现数字 0");
    }

    #[test]
    fn tooltip_carries_product_name_and_hotkey() {
        let text = tooltip("Ctrl+Alt+Space", 2);
        assert!(text.starts_with("Slip"), "{text}");
        assert!(text.contains("2 项待办"), "{text}");
        assert!(text.contains("Ctrl+Alt+Space"), "{text}");
    }

    #[test]
    fn tooltip_reports_missing_hotkey_explicitly() {
        // 热键注册失败时不能装作没事——用户会发现"按了没反应"，得能自己看出原因。
        let text = tooltip("", 0);
        assert!(text.contains("热键未注册"), "{text}");
        assert!(text.contains("暂无待办"), "{text}");
    }
}
