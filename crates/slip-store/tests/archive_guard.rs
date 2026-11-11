//! 归档前置条件：未完成的纸条不得被归档。
//!
//! 这条规则原本只有 `slip-domain` 的单元测试守着。这里从 store 侧再钉一次：
//! 走的是真实路径（`apply` → `apply_patch` → 领域校验），顺带证明"**拒绝即不写**"——
//! 如果哪天有人把领域校验挪到写盘之后，文件会先被改再报错，这条会立刻红。

use std::path::{Path, PathBuf};

use slip_domain::{Mutation, Patch, Slip, SlipId};
use slip_store::{MarkdownStore, Store, StoreError};
use time::macros::datetime;

fn vault() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("temp vault");
    std::fs::write(dir.path().join("inbox.md"), b"").expect("seed empty inbox");
    dir
}

fn inbox(dir: &Path) -> PathBuf {
    dir.join("inbox.md")
}

fn read(path: &Path) -> Vec<u8> {
    std::fs::read(path).expect("read inbox")
}

fn record(id: &str, text: &str) -> String {
    format!("- [ ] {text} <!-- slip:v1;id={id};created=2026-11-10T00:00:00Z -->\n")
}

fn slip_id(raw: &str) -> SlipId {
    SlipId::parse_strict(raw).expect("valid slip id")
}

#[allow(dead_code)]
fn slip(id: &str, text: &str) -> Slip {
    Slip {
        id: slip_id(id),
        text: text.to_owned(),
        done: false,
        locked: false,
        created_at: datetime!(2026-11-10 0:00 UTC),
        done_at: None,
        archived_at: None,
    }
}

/// 归档一条未完成的纸条：必须被拒绝，且文件一个字节都不动。
#[test]
fn archiving_an_undone_slip_is_rejected_and_writes_nothing() {
    let dir = vault();
    let path = inbox(dir.path());
    let id = "01a1c7e0-0000-7000-8000-000000000031";
    std::fs::write(&path, record(id, "还没完成的一条")).expect("seed");
    let before = read(&path);

    let store = MarkdownStore::new(&path);
    let outcome = store.apply(&[Mutation::Patch {
        id: slip_id(id),
        patch: Patch::Archive {
            at: datetime!(2026-11-11 0:00 UTC),
        },
    }]);

    assert!(
        matches!(outcome.result, Err(StoreError::Domain(_))),
        "归档未完成条目必须被领域层拒绝，实际是 {:?}",
        outcome.result
    );
    assert_eq!(read(&path), before, "被拒绝的归档不得改动文件");

    let snap = store.snapshot();
    let stored = snap
        .slips
        .iter()
        .find(|s| s.id == slip_id(id))
        .expect("条目仍在");
    assert!(!stored.done, "不应被顺带标记完成");
    assert!(stored.archived_at.is_none(), "不应留下归档时间");
}
