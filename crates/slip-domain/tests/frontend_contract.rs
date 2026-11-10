//! 前端契约测试：`ui/src/lib/Capture.svelte` 里的上限必须与领域层**同一个值**。
//!
//! 为什么需要它：这两个数字是同一条规则的两种表达。后端在 `normalize_text` /
//! `split_drafts` 里拒绝超限，前端只是提前提示"第几行超了多少"。一旦漂移，
//! 症状是「前端放行、后端拒绝」或者「前端误拦」——两者都不报错，只是让用户
//! 困惑，靠人眼 review 抓不住。
//!
//! 这里的耦合是刻意的：共享口径本来就需要一个显式锚点，而不是两边各写一份
//! 数字然后祈祷它们不变。改任何一边，这条测试都会红。

use std::fs;
use std::path::PathBuf;

fn capture_source() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../ui/src/lib/Capture.svelte");
    fs::read_to_string(&path).unwrap_or_else(|error| panic!("读不到 {}：{error}", path.display()))
}

/// 从源码里取 `const NAME = <整数>;` 的数值。找不到返回 `None`，
/// 这样"常量被改名/删掉"和"数值变了"会给出可区分的失败信息。
fn declared_number(source: &str, name: &str) -> Option<u64> {
    let needle = format!("const {name} =");
    let start = source.find(&needle)? + needle.len();
    let rest = &source[start..];
    let end = rest.find(';')?;
    rest[..end].trim().parse().ok()
}

#[test]
fn frontend_limits_match_the_domain() {
    let source = capture_source();

    assert_eq!(
        declared_number(&source, "MAX_CODEPOINTS"),
        Some(slip_domain::MAX_TEXT_CODEPOINTS as u64),
        "Capture.svelte 的 MAX_CODEPOINTS 必须等于 slip_domain::MAX_TEXT_CODEPOINTS"
    );

    assert_eq!(
        declared_number(&source, "MAX_LINES"),
        Some(slip_domain::MAX_LINES as u64),
        "Capture.svelte 的 MAX_LINES 必须等于 slip_domain::MAX_LINES"
    );
}
