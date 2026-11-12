//! 前端状态生命周期契约：`ui/src/App.svelte` 里的"临时视图状态"必须在约定的时机清掉。
//!
//! 为什么要读源码：这些状态活在 Svelte 组件里，没有可单测的纯函数入口。端到端
//! （`tools/cdp-probe.mjs` 第 24 节）能验行为，但它需要交互式桌面与可启动的
//! `slip-shell.exe`，在 Smart App Control 会拦新产物的机器上跑不了。这里补一层
//! **结构守卫**：函数体里少了清理语句就红，防止重构时静默删掉。
//!
//! 已知局限（诚实标注）：
//! - 语句按行匹配，**注释行会被过滤**——把 `showArchived = false;` 注释掉必须让测试红，
//!   否则子串匹配会把"删掉的清理"当成"还在"；
//! - 但仍不检查语句是否被条件分支包住，也不验证运行时行为。行为级验证归端到端，
//!   本测试的职责只是"清理语句以可执行的形式存在"。

use std::fs;
use std::path::PathBuf;

fn app_source() -> String {
    source_for("ui/src/App.svelte")
}

fn slip_list_source() -> String {
    source_for("ui/src/lib/SlipList.svelte")
}

fn source_for(relative: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("../../{relative}"));
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

/// 断言 `function` 的函数体里存在一条**可执行**语句，返回其行号（函数体内 1-based）。
///
/// 按行匹配而不是子串匹配：`// showArchived = false;` 在 `str::contains` 下仍然通过，
/// 这是结构守卫最容易产生的假阴性。这里跳过空行与 `trim` 后以 `//` 开头的行，
/// 并要求整行（`trim` 后）恰好等于 `statement`。
fn assert_body_has_statement(function: &str, source: &str, statement: &str) -> usize {
    let body = function_body(source, function)
        .unwrap_or_else(|| panic!("找不到函数 {function}：签名或闭合大括号变了，请同步本测试"));
    for (index, line) in body.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("//") {
            continue;
        }
        if trimmed == statement {
            return index + 1;
        }
    }
    panic!(
        "{function} 的函数体里缺少可执行语句 `{statement}`（注释行不算）。临时视图状态不跨\
         唤出、整体替换内容必须清空基于旧内容的派生状态（见 AGENTS.md「状态生命周期」）。\
         函数体：\n{body}"
    );
}

/// 断言 `function` 的函数体里有**可执行代码**包含 `needle`，返回其行号（函数体内 1-based）。
///
/// 与 `assert_body_has_statement` 同一套防假阴性规则：注释行不算。用于"调用必须发生"
/// 这类断言——例如纯函数测试只能证明 `shouldDropPendingIntent` 本身正确，证明不了
/// `handleToggle` 真的调用了它；把调用注释掉必须让本测试红。
fn assert_body_contains_code(function: &str, source: &str, needle: &str) -> usize {
    let body = function_body(source, function)
        .unwrap_or_else(|| panic!("找不到函数 {function}：签名或闭合大括号变了，请同步本测试"));
    for (index, line) in body.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("//") {
            continue;
        }
        if trimmed.contains(needle) {
            return index + 1;
        }
    }
    panic!(
        "{function} 的函数体里缺少可执行代码 `{needle}`（注释行不算）。这条守卫证明调用\
         真的发生，而不只是纯函数本身正确（见 AGENTS.md「状态生命周期」）。函数体：\n{body}"
    );
}

/// `summon()` = 新一次捕获会话，不继承上一轮的浏览位置。
///
/// AGENTS.md「状态生命周期」同一张表覆盖：`filter` / `filtering`、`showArchived`、
/// `showBackups` / `pendingRestore`、`notice` 与 `undoTrail` 都必须在这里清。
#[test]
fn summon_resets_the_archived_and_backup_views() {
    let source = app_source();
    let summon = "function summon() {";
    for statement in [
        "notice = null;",
        "undoTrail = null;",
        "filter = \"\";",
        "filtering = false;",
        "showArchived = false;",
        "showBackups = false;",
        "pendingRestore = null;",
        // 未落盘的勾选意图只属于当次会话：上一轮失败留下的意图会让下一次唤出的
        // 第一下点击从旧状态反转（勾选要按两下）。
        "pendingDone = {};",
        // 按 id 的在途计数也只属于当次会话。
        "pendingToggleCounts = {};",
        // 只读预览与备份列表不跨唤出。
        "preview = null;",
        "backups = [];",
        // 二次确认武装态（含彻底删除）绝不能跨唤出复用：窗口失焦收起再唤出，
        // 同一行仍是武装态时，下一次单击会直接执行不可逆删除。
        "for (const key of Object.keys(armed)) armed[key] = null;",
    ] {
        assert_body_has_statement(summon, &source, statement);
    }
    // 武装定时器也要清，否则到点会把新会话的键写回 null（无害但不干净）。
    assert_body_contains_code(summon, &source, "clearTimeout(armTimers[key]);");
}

/// 重新载入 = 换了一份内容，挂着旧内容的撤销提示必须过期。
#[test]
fn reload_clears_the_undo_notice() {
    let source = app_source();
    assert_body_has_statement("async function handleReload() {", &source, "notice = null;");
}

/// 从备份恢复 = 换了一份内容，旧撤销提示同样必须过期。
#[test]
fn restore_backup_clears_the_undo_notice() {
    let source = app_source();
    assert_body_has_statement(
        "async function handleRestore(backup) {",
        &source,
        "notice = null;",
    );
}

/// 隔离并重建 = 换了一份空内容，旧撤销提示同样必须过期。
#[test]
fn isolate_and_reset_clears_the_undo_notice() {
    let source = app_source();
    assert_body_has_statement(
        "async function handleIsolate() {",
        &source,
        "notice = null;",
    );
}

/// 勾选失败后必须删除那条未落盘的意图。
///
/// 纯函数测试（`tools/check-ui-logic.mjs` 的 `shouldDropPendingIntent` 五条）只能证明
/// 判定本身正确，证明不了 `handleToggle` 真的调用了它。把 `finally` 里的判定去掉，
/// 意图就会永久残留 → 下一次点击从"没发生过的状态"反转 → 勾选要按两下。
#[test]
fn handle_toggle_drops_the_pending_intent_through_the_pure_rule() {
    let source = app_source();
    assert_body_contains_code(
        "async function handleToggle(slip) {",
        &source,
        "shouldDropPendingIntent(",
    );
}

/// 损坏预览是只读证据：弹窗开着时窗口级快捷键必须全部让路。
#[test]
fn corrupt_preview_blocks_window_shortcuts() {
    let source = app_source();
    assert_body_has_statement(
        "function handleKeydown(event) {",
        &source,
        "if (preview) return;",
    );
    // Tab 也必须被拦在弹窗内，否则焦点会落到弹窗背后被遮住的控件上。
    assert_body_contains_code(
        "function handleKeydownCapture(event) {",
        &source,
        "event.key === \"Tab\" && preview",
    );
}

/// 撤销必须排在已提交的变更之后：勾选还在飞时直接发 undo 会撤错那一条。
#[test]
fn undo_goes_through_the_serial_queue() {
    let source = app_source();
    assert_body_contains_code("async function handleUndo() {", &source, "mutating.run(");
}

/// 备份列表的响应必须按代数校验：快速开→关→开时旧响应不许覆盖新结果。
#[test]
fn backup_list_response_is_guarded_by_a_generation_token() {
    let source = app_source();
    assert_body_contains_code("async function toggleBackups() {", &source, "backupsToken");
}

/// 编辑提交失败时不许切到另一条：Tab 与 `beginEdit` 都必须等返回值。
///
/// 旧写法 `commitEdit().then(() => beginEdit(...))` 在失败时照样切目标，
/// 而 `beginEdit` 里又 `void commitEdit()`——同一份草稿发两遍，最后被覆盖丢失。
#[test]
fn edit_switch_waits_for_commit_success() {
    let list = slip_list_source();
    assert_body_contains_code(
        "async function handleKeydown(event) {",
        &list,
        "if (!(await commitEdit())) return;",
    );
    assert_body_contains_code(
        "async function beginEdit(slip) {",
        &list,
        "await commitEdit()",
    );
}

/// 在编辑框里双击选词不能把草稿重置成原文。
///
/// 双击事件会从输入框冒泡到整行的 `ondblclick`；`beginEdit` 若不在"已经在编辑
/// 这一条"时短路，就会执行 `draft = slip.text`，把用户刚打的字静默抹掉。
#[test]
fn editing_input_double_click_does_not_reset_the_draft() {
    let list = slip_list_source();
    assert_body_has_statement(
        "async function beginEdit(slip) {",
        &list,
        "if (editingId === slip.id) return;",
    );
    assert!(
        list.contains("ondblclick={(event) => event.stopPropagation()}"),
        "编辑框必须阻止双击冒泡到整行：{list}"
    );
}

/// 提交在飞时并发调用必须共享同一个 Promise，而不是直接放行。
#[test]
fn edit_commit_is_shared_while_in_flight() {
    let list = slip_list_source();
    assert_body_contains_code(
        "async function commitEdit() {",
        &list,
        "if (commitInFlight) return commitInFlight;",
    );
}

/// 整库替换（重载）必须退出归档抽屉：旧内容上的视图状态不再有意义。
#[test]
fn whole_file_replacement_exits_the_archived_view() {
    let source = app_source();
    for function in [
        "async function handleReload() {",
        "async function handleRestore(backup) {",
        "async function handleIsolate() {",
    ] {
        assert_body_has_statement(function, &source, "showArchived = false;");
        assert_body_has_statement(function, &source, "preview = null;");
    }
}

/// Noop 是 Applied，但前端不能因此挂"可撤销"的提示。
#[test]
fn noop_does_not_pretend_to_be_undoable() {
    let source = app_source();
    for function in [
        "async function handleToggle(slip) {",
        "async function handleEdit(slip, text) {",
        "async function handleArchive(slip) {",
        "async function handleCompleteMany() {",
    ] {
        assert_body_contains_code(function, &source, "ack.changed !== false");
    }
}

/// 所有内容变更命令必须走同一条串行队列（撤销排在已提交变更之后）。
#[test]
fn all_content_mutations_go_through_the_queue() {
    let source = app_source();
    for function in [
        "async function handleEdit(slip, text) {",
        "async function handleArchive(slip) {",
        "async function handleUnarchive(slip) {",
        "async function handlePurge(slip) {",
        "async function handleRestore(backup) {",
        "async function handleIsolate() {",
        "async function handleReload() {",
        "async function handleLock(slip) {",
        "async function handlePin(slip) {",
    ] {
        assert_body_contains_code(function, &source, "mutating.run(");
    }
}

/// 归档数变成 0 时，「返回列表」仍必须存在（否则用户被留在空抽屉里）。
#[test]
fn archived_empty_state_still_has_an_exit() {
    let source = app_source();
    assert!(
        source.contains("{#if archivedSlips.length > 0 || showArchived}"),
        "「返回列表」必须由 showArchived 决定，而不能只看归档条数"
    );
}
