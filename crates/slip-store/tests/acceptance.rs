//! 跨 crate 集成验收。
//!
//! 这些测试刻意走**真实的临时目录与真实文件**，不用 mock：
//! 要验证的是"磁盘上到底发生了什么"，mock 掉磁盘就什么也没验证。
//!
//! 命名与文档对应：
//! - `format_closure_*`      §9.1-1
//! - `clock_rollback_*`      §9.1-1 / I22
//! - `metadata_visibility_*` §9.1-2 / I24
//! - `recovery_*`            §9.1-3 / I20
//! - `commit_point_*`        §9.1 提交点
//!
//! 注意：本文件只依赖公开 API；不要为了测试去放宽可见性。

use std::path::{Path, PathBuf};

use slip_domain::{Mutation, Patch, Slip, SlipId};
use slip_store::{Availability, MarkdownStore, RecoveryState, Store, StoreError};
use time::OffsetDateTime;
use time::macros::datetime;

const ID: &str = "019a9f3c-2b70-7d84-a1c6-8e3f5b0d2741";
const ID2: &str = "019a9f3c-2b70-7d84-a1c6-8e3f5b0d2742";

fn vault() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("slip-vault-")
        .tempdir()
        .expect("create temp vault")
}

fn inbox(dir: &Path) -> PathBuf {
    dir.join("inbox.md")
}

fn read(path: &Path) -> Vec<u8> {
    std::fs::read(path).expect("read inbox")
}

fn slip(id: &str, text: &str, created_at: OffsetDateTime) -> Slip {
    Slip {
        id: SlipId::parse_strict(id).expect("valid id"),
        text: text.to_owned(),
        done: false,
        created_at,
        done_at: None,
        archived_at: None,
        locked: false,
    }
}

fn record(id: &str, text: &str) -> String {
    format!("- [ ] {text} <!-- slip:v1;id={id};created=2026-11-10T11:49:14+08:00 -->")
}

fn corrupt_files(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .expect("read dir")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.contains(".corrupt-"))
        })
        .collect()
}

// ---------------------------------------------------------------- 格式闭包

#[test]
fn format_closure_append_is_visible_and_round_trips() {
    let dir = vault();
    let path = inbox(dir.path());
    let store = MarkdownStore::new(&path);

    let outcome = store.apply(&[Mutation::Append(slip(
        ID,
        "买发票",
        datetime!(2026-11-10 11:49:14 +08:00),
    ))]);

    assert!(outcome.is_applied(), "append must commit");
    let bytes = read(&path);
    let text = String::from_utf8(bytes.clone()).expect("utf-8");
    assert!(text.starts_with("- [ ] "), "got {text:?}");
    assert!(text.contains("买发票"));
    assert!(text.contains("<!-- slip:v1;id="));
    assert!(text.contains(";created="));
    assert!(text.trim_end().ends_with("-->"), "marker must be intact");

    // 重新载入必须得到同一条内容——格式必须能被自己读回。
    let reloaded = store.reload();
    assert!(reloaded.is_applied());
    assert_eq!(reloaded.snapshot.slips.len(), 1);
    assert_eq!(reloaded.snapshot.slips[0].text, "买发票");
    assert_eq!(
        reloaded.snapshot.slips[0].id,
        SlipId::parse_strict(ID).unwrap()
    );
}

#[test]
fn format_closure_second_append_separates_without_trailing_newline() {
    let dir = vault();
    let path = inbox(dir.path());
    let store = MarkdownStore::new(&path);

    store.apply(&[Mutation::Append(slip(
        ID,
        "a",
        datetime!(2026-11-10 11:49:14 +08:00),
    ))]);
    store.apply(&[Mutation::Append(slip(
        ID2,
        "b",
        datetime!(2026-11-10 11:49:15 +08:00),
    ))]);

    let bytes = read(&path);
    // 全新空文件没有末尾终止符，追加后也不应凭空多出一个。
    assert_eq!(
        bytes.iter().filter(|byte| **byte == b'\n').count(),
        1,
        "two records need exactly one separator"
    );
    assert!(!bytes.ends_with(b"\n"));

    let reloaded = store.reload();
    assert_eq!(reloaded.snapshot.slips.len(), 2);
    assert_eq!(reloaded.snapshot.slips[0].text, "a");
    assert_eq!(reloaded.snapshot.slips[1].text, "b");
}

#[test]
fn format_closure_patch_preserves_untouched_lines_byte_for_byte() {
    let dir = vault();
    let path = inbox(dir.path());

    let header = b"# Inbox\r\n";
    let plain = b"not a record -- just prose with trailing spaces   \r\n";
    let original_record = record(ID, "旧").into_bytes();
    let mut seed = Vec::new();
    seed.extend_from_slice(header);
    seed.extend_from_slice(&original_record);
    seed.extend_from_slice(b"\r\n");
    seed.extend_from_slice(plain);
    std::fs::write(&path, &seed).expect("seed inbox");

    let store = MarkdownStore::new(&path);
    assert_eq!(store.snapshot().slips.len(), 1);

    let outcome = store.apply(&[Mutation::Patch {
        id: SlipId::parse_strict(ID).unwrap(),
        patch: Patch::SetText("新".to_owned()),
    }]);
    assert!(outcome.is_applied());

    let after = read(&path);
    // 未修改行必须逐字节保留：包括 CRLF、行尾空格、以及那条空白记录行。
    assert!(after.starts_with(header), "header bytes changed");
    assert!(
        after.windows(plain.len()).any(|window| window == plain),
        "untouched prose line changed"
    );
    let text = String::from_utf8(after).unwrap();
    assert!(text.contains("新"), "patched text missing");
    assert!(!text.contains("旧"), "old text still present");
}

// ---------------------------------------------------------------- 时钟回拨

#[test]
fn clock_rollback_is_accepted_and_stored_verbatim() {
    // v11 刻意**不做**时钟合法性检查：回拨是用户的真实处境，不是数据错误。
    let dir = vault();
    let path = inbox(dir.path());
    let store = MarkdownStore::new(&path);

    let newer = store.apply(&[Mutation::Append(slip(
        ID,
        "后写的但是时间更晚",
        datetime!(2026-11-10 12:00:00 +08:00),
    ))]);
    assert!(newer.is_applied());

    let rollback = store.apply(&[Mutation::Append(slip(
        ID2,
        "时间回拨到过去",
        datetime!(2020-01-01 00:00:00 +08:00),
    ))]);
    assert!(rollback.is_applied(), "clock rollback must not be rejected");

    let text = String::from_utf8(read(&path)).unwrap();
    assert!(text.contains("2020-01-01T00:00:00+08:00"));
    // 顺序是**追加顺序**，不是时间顺序。
    assert!(text.find("后写的但是时间更晚").unwrap() < text.find("时间回拨到过去").unwrap());
}

// ---------------------------------------------------------------- 冲突

#[test]
fn conflict_leaves_file_byte_identical() {
    let dir = vault();
    let path = inbox(dir.path());
    let store = MarkdownStore::new(&path);

    store.apply(&[Mutation::Append(slip(
        ID,
        "a",
        datetime!(2026-11-10 11:49:14 +08:00),
    ))]);

    // 外部编辑器改了文件。
    let mut external = read(&path);
    external.extend_from_slice("\r\n外部新增的一行".as_bytes());
    std::fs::write(&path, &external).expect("external edit");

    let outcome = store.apply(&[Mutation::Append(slip(
        ID2,
        "b",
        datetime!(2026-11-10 11:49:15 +08:00),
    ))]);

    assert!(
        matches!(outcome.result, Err(StoreError::Conflict { .. })),
        "expected conflict, got {:?}",
        outcome.result
    );
    // 冲突时**绝不写字节**：外部那版必须原样留着。
    assert_eq!(read(&path), external);
    assert!(outcome.snapshot.needs_reload);
}

// ---------------------------------------------------------------- 元数据可见性

#[test]
fn metadata_visibility_file_exists_without_explicit_flush() {
    let dir = vault();
    let path = inbox(dir.path());
    let store = MarkdownStore::new(&path);

    store.apply(&[Mutation::Append(slip(
        ID,
        "立即可见",
        datetime!(2026-11-10 11:49:14 +08:00),
    ))]);

    // 没有 flush / close / drop：apply 返回时字节必须已经落到路径上。
    let metadata = std::fs::metadata(&path).expect("file must exist");
    assert!(metadata.len() > 0, "file is empty right after apply");

    // 用一条全新的读取器（不是 store）确认内容确实在磁盘上。
    let raw = std::fs::read_to_string(&path).unwrap();
    assert!(raw.contains("立即可见"));
}

#[test]
fn noop_patch_does_not_rewrite_the_file() {
    let dir = vault();
    let path = inbox(dir.path());
    std::fs::write(&path, format!("{}\n", record(ID, "旧"))).expect("seed");
    let store = MarkdownStore::new(&path);

    let before_bytes = read(&path);
    let before_rev = store.snapshot().content_rev;

    let outcome = store.apply(&[Mutation::Patch {
        id: SlipId::parse_strict(ID).unwrap(),
        patch: Patch::SetText("旧".to_owned()),
    }]);

    assert!(outcome.is_applied(), "noop is still a success");
    assert_eq!(read(&path), before_bytes, "noop must not touch bytes");
    assert_eq!(
        store.snapshot().content_rev,
        before_rev,
        "noop must not bump content_rev"
    );
}

// ---------------------------------------------------------------- 恢复

#[test]
fn recovery_corrupt_startup_is_degraded_and_read_only() {
    let dir = vault();
    let path = inbox(dir.path());
    // 记录候选行（`- [` 前缀 + ` <!-- slip:`），但 id 不是 UUIDv7。
    std::fs::write(
        &path,
        b"- [ ] broken <!-- slip:v1;id=not-a-uuid;created=2026-11-10T11:49:14+08:00 -->\n",
    )
    .expect("seed corrupt file");

    let store = MarkdownStore::new(&path);
    let snapshot = store.snapshot();
    assert_eq!(
        snapshot.health.availability,
        Availability::Degraded,
        "invalid record must degrade the vault"
    );
    assert!(!snapshot.health.is_writable());

    let outcome = store.apply(&[Mutation::Append(slip(
        ID2,
        "不应该被写入",
        datetime!(2026-11-10 11:49:14 +08:00),
    ))]);
    assert!(outcome.result.is_err(), "degraded vault must refuse writes");
    assert!(
        !String::from_utf8_lossy(&read(&path)).contains("不应该被写入"),
        "no byte may be written while degraded"
    );
}

#[test]
fn recovery_isolate_and_reset_returns_to_normal() {
    let dir = vault();
    let path = inbox(dir.path());
    std::fs::write(
        &path,
        b"- [ ] broken <!-- slip:v1;id=not-a-uuid;created=2026-11-10T11:49:14+08:00 -->\n",
    )
    .expect("seed corrupt file");

    let store = MarkdownStore::new(&path);
    assert!(corrupt_files(dir.path()).is_empty());

    let outcome = store.isolate_and_reset();
    assert!(outcome.result.is_ok(), "isolation must succeed");
    assert_eq!(outcome.snapshot.recovery, RecoveryState::Normal);
    assert!(outcome.snapshot.health.is_writable());

    // 原字节被搬到旁路文件，主文件重新变空。
    assert!(read(&path).is_empty(), "reset file must be empty");
    assert_eq!(
        corrupt_files(dir.path()).len(),
        1,
        "corrupt copy must exist"
    );

    // 恢复之后必须能正常写入。
    let after = store.apply(&[Mutation::Append(slip(
        ID2,
        "恢复后可以写",
        datetime!(2026-11-10 11:49:14 +08:00),
    ))]);
    assert!(after.is_applied());
    assert!(String::from_utf8_lossy(&read(&path)).contains("恢复后可以写"));
}

#[test]
fn recovery_isolate_refuses_when_healthy() {
    // 健康状态下的 isolate 是误操作，必须拒绝而不是把好数据搬走。
    let dir = vault();
    let path = inbox(dir.path());
    std::fs::write(&path, format!("{}\n", record(ID, "好数据"))).expect("seed");
    let store = MarkdownStore::new(&path);
    let before = read(&path);

    let outcome = store.isolate_and_reset();
    assert!(
        matches!(outcome.result, Err(StoreError::NotDegraded)),
        "got {:?}",
        outcome.result
    );
    assert_eq!(read(&path), before, "healthy file must not be moved");
    assert!(corrupt_files(dir.path()).is_empty());
}

// ---------------------------------------------------------------- 陈旧视图

#[test]
fn stale_patch_requires_reload_and_writes_nothing() {
    // 用户对着旧快照点了"完成"，但那条纸条已被外部删掉。
    // 这是**视图陈旧**，不是 ID 非法——不能报 "invalid slip id"。
    let dir = vault();
    let path = inbox(dir.path());
    std::fs::write(&path, format!("{}\n", record(ID, "仍然存在"))).expect("seed");
    let store = MarkdownStore::new(&path);
    let before = read(&path);

    let outcome = store.apply(&[Mutation::Patch {
        // 合法 UUIDv7，但文档里没有这一条。
        id: SlipId::parse_strict(ID2).unwrap(),
        patch: Patch::SetText("改了".to_owned()),
    }]);

    assert!(
        matches!(outcome.result, Err(StoreError::NeedsReload)),
        "stale view must ask for reload, got {:?}",
        outcome.result
    );
    assert_eq!(read(&path), before, "no byte may be written");

    // 关键：不置 needs_reload。文档没变，后续正常写入不该被冻结。
    assert!(!outcome.snapshot.needs_reload);
    let after = store.apply(&[Mutation::Append(slip(
        ID2,
        "后续写入",
        datetime!(2026-11-10 11:49:14 +08:00),
    ))]);
    assert!(
        after.is_applied(),
        "a stale patch must not freeze the vault: {:?}",
        after.result
    );
}

// ---------------------------------------------------------------- 提交点

#[test]
fn commit_point_committed_write_is_reflected_in_revision() {
    let dir = vault();
    let path = inbox(dir.path());
    let store = MarkdownStore::new(&path);

    let before = store.snapshot().content_rev;
    let outcome = store.apply(&[Mutation::Append(slip(
        ID,
        "一次提交",
        datetime!(2026-11-10 11:49:14 +08:00),
    ))]);

    assert!(outcome.is_applied());
    assert!(
        store.snapshot().content_rev > before,
        "committed write must advance content_rev"
    );
    assert_eq!(outcome.result.unwrap(), store.snapshot().content_rev);

    // 一次提交恰好留一代备份。
    let backups = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.contains(".bak-"))
        })
        .count();
    assert_eq!(backups, 1, "one commit must leave exactly one backup");
}
