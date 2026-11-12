//! 前端状态生命周期契约：`ui/src/App.svelte` 里的"临时视图状态"必须在约定的时机清掉。
//!
//! 为什么要读源码：这些状态活在 Svelte 组件里，没有可单测的纯函数入口。端到端
//! （`tools/cdp-probe.mjs` 第 24 节）能验行为，但它需要交互式桌面与可启动的
//! `slip-shell.exe`，在 Smart App Control 会拦新产物的机器上跑不了。这里补一层
//! **结构守卫**：函数体里少了清理语句就红，防止重构时静默删掉。
//!
//! 已知局限（诚实标注）：只检查语句存在，不检查它是否被条件分支包住。行为级验证
//! 仍归端到端；本测试的职责是"清理语句不许消失"。

use std::fs;
use std::path::PathBuf;

fn app_source() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../ui/src/App.svelte");
    fs::read_to_string(&path).unwrap_or_else(|error| panic!("读不到 {}：{error}", path.display()))
}

/// 取 `signature` 对应的函数体（不含签名与闭合大括号）。
///
/// 依赖仓库强制的 prettier 风格：顶层函数以两格缩进闭合。相比手写大括号计数，
/// 这样不会把模板字符串里的 `${...}` 数错。找不到签名或闭合行时返回 `None`，
/// 让"函数被改名/移动"与"清理语句被删"给出可区分的失败信息。
fn function_body<'a>(source: &'a str, signature: &str) -> Option<&'a str> {
    let start = source.find(signature)? + signature.len();
    let rest = &source[start..];
    let end = rest.find("\n  }\n")?;
    Some(&rest[..end])
}

fn assert_body_contains(function: &str, source: &str, needle: &str) {
    let body = function_body(source, function)
        .unwrap_or_else(|| panic!("找不到函数 {function}：签名或闭合大括号变了，请同步本测试"));
    assert!(
        body.contains(needle),
        "{function} 的函数体里缺少 `{needle}`。临时视图状态不跨唤出、整体替换内容必须清空\
         基于旧内容的派生状态（见 AGENTS.md「状态生命周期」）。函数体：\n{body}"
    );
}

/// `summon()` = 新一次捕获会话，不继承上一轮的浏览位置。
#[test]
fn summon_resets_the_archived_and_backup_views() {
    let source = app_source();
    let summon = "function summon() {";
    for statement in [
        "showArchived = false;",
        "showBackups = false;",
        "pendingRestore = null;",
    ] {
        assert_body_contains(summon, &source, statement);
    }
}

/// 重新载入 = 换了一份内容，挂着旧内容的撤销提示必须过期。
#[test]
fn reload_clears_the_undo_notice() {
    let source = app_source();
    assert_body_contains("async function handleReload() {", &source, "notice = null;");
}

/// 从备份恢复 = 换了一份内容，旧撤销提示同样必须过期。
#[test]
fn restore_backup_clears_the_undo_notice() {
    let source = app_source();
    assert_body_contains(
        "async function handleRestore(backup) {",
        &source,
        "notice = null;",
    );
}

/// 隔离并重建 = 换了一份空内容，旧撤销提示同样必须过期。
#[test]
fn isolate_and_reset_clears_the_undo_notice() {
    let source = app_source();
    assert_body_contains(
        "async function handleIsolate() {",
        &source,
        "notice = null;",
    );
}
