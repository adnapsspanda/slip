//! 端到端流水线：真实磁盘 + 真实 store + 真实调度器。
//!
//! 这里刻意**不用 mock**。领域层、事务层、调度层各自都有单测，但"用户按一次
//! 回车之后磁盘上到底多了什么"只有让真实组件串起来才能回答。
//!
//! 唯一没覆盖的是 `src-tauri` 里那几行命令包装——它们只做类型搬运。

use std::sync::Arc;

use slip_app::AckResult;
use slip_app::dispatch::Dispatcher;
use slip_app::presenter::Presenter;
use slip_domain::{Mutation, Patch, Slip, SlipId};
use slip_store::MarkdownStore;
use slip_store::Store;
use slip_store::prefs::JsonPrefsStore;
use time::OffsetDateTime;

fn slip(text: &str) -> Slip {
    Slip {
        id: SlipId::new_v7(),
        text: text.to_owned(),
        done: false,
        created_at: OffsetDateTime::now_utc(),
        done_at: None,
        archived_at: None,
    }
}

#[tokio::test(flavor = "current_thread")]
async fn capture_writes_through_to_disk_and_back_to_the_snapshot() {
    let dir = tempfile::tempdir().expect("temp vault");
    let inbox = dir.path().join("inbox.md");

    let store = Arc::new(MarkdownStore::new(&inbox));
    let prefs = Arc::new(JsonPrefsStore::new(dir.path().join("prefs.json")));
    let presenter = Presenter::with_view(store.snapshot(), Default::default());
    let dispatcher = Dispatcher::spawn(store, prefs, presenter, 8);

    let created = slip("买发票");
    let ack = dispatcher
        .apply(String::from("r1"), vec![Mutation::Append(created.clone())])
        .await;
    assert_eq!(ack.result, AckResult::Applied, "{ack:?}");

    // 1. 磁盘上真的有了这一行，且格式能被自己读回。
    let raw = std::fs::read_to_string(&inbox).expect("read inbox");
    assert!(raw.contains("买发票"), "disk content: {raw:?}");
    assert!(raw.contains("<!-- slip:v1;id="), "marker missing: {raw:?}");

    // 2. 快照如实反映了它，且 ui_rev 已经前进。
    let snapshot = dispatcher.get_snapshot().await.expect("snapshot");
    assert_eq!(snapshot.slips.len(), 1);
    assert_eq!(snapshot.slips[0].text, "买发票");
    assert_eq!(snapshot.slips[0].id, created.id.to_canonical_string());
    assert!(!snapshot.slips[0].done);
    assert!(snapshot.ui_rev >= 2);

    // 3. 勾选之后，磁盘与快照同时变。
    let ack = dispatcher
        .apply(
            String::from("r2"),
            vec![Mutation::Patch {
                id: created.id,
                patch: Patch::SetDone {
                    done: true,
                    at: OffsetDateTime::now_utc(),
                },
            }],
        )
        .await;
    assert_eq!(ack.result, AckResult::Applied, "{ack:?}");

    let raw = std::fs::read_to_string(&inbox).expect("read inbox");
    assert!(raw.contains("- [x] "), "checkbox not updated: {raw:?}");
    assert!(raw.contains(";done_at="), "done_at not written: {raw:?}");
    assert!(dispatcher.get_snapshot().await.unwrap().slips[0].done);

    // 4. 重复勾选是 Noop：不写盘、不涨版本。
    let before = std::fs::read(&inbox).unwrap();
    let ack = dispatcher
        .apply(
            String::from("r3"),
            vec![Mutation::Patch {
                id: created.id,
                patch: Patch::SetDone {
                    done: true,
                    at: OffsetDateTime::now_utc(),
                },
            }],
        )
        .await;
    assert_eq!(ack.result, AckResult::Applied, "noop is still a success");
    assert_eq!(std::fs::read(&inbox).unwrap(), before, "noop rewrote bytes");

    dispatcher.shutdown().await;
}

#[tokio::test(flavor = "current_thread")]
async fn restarting_the_shell_reads_the_same_content_back() {
    let dir = tempfile::tempdir().expect("temp vault");
    let inbox = dir.path().join("inbox.md");

    {
        let store = Arc::new(MarkdownStore::new(&inbox));
        let prefs = Arc::new(JsonPrefsStore::new(dir.path().join("prefs.json")));
        let presenter = Presenter::with_view(store.snapshot(), Default::default());
        let dispatcher = Dispatcher::spawn(store, prefs, presenter, 8);
        let ack = dispatcher
            .apply(String::from("r1"), vec![Mutation::Append(slip("一次写入"))])
            .await;
        assert_eq!(ack.result, AckResult::Applied);
        dispatcher.shutdown().await;
    }

    // 模拟重新启动：全新的 store 读同一个文件。
    let store = Arc::new(MarkdownStore::new(&inbox));
    let presenter = Presenter::with_view(store.snapshot(), Default::default());
    let prefs = Arc::new(JsonPrefsStore::new(dir.path().join("prefs.json")));
    let dispatcher = Dispatcher::spawn(store, prefs, presenter, 8);

    let snapshot = dispatcher.get_snapshot().await.unwrap();
    assert_eq!(snapshot.slips.len(), 1);
    assert_eq!(snapshot.slips[0].text, "一次写入");
    dispatcher.shutdown().await;
}

#[tokio::test(flavor = "current_thread")]
async fn capture_of_several_lines_creates_several_slips() {
    let dir = tempfile::tempdir().expect("temp vault");
    let inbox = dir.path().join("inbox.md");
    let store = Arc::new(MarkdownStore::new(&inbox));
    let prefs = Arc::new(JsonPrefsStore::new(dir.path().join("prefs.json")));
    let presenter = Presenter::with_view(store.snapshot(), Default::default());
    let dispatcher = Dispatcher::spawn(store, prefs, presenter, 8);

    // 外壳把多行文本交给 split_drafts，这里直接复现那条路径。
    let now = OffsetDateTime::now_utc();
    let drafts = slip_domain::split_drafts("第一件事\n第二件事\n\n第三件事", now).expect("split");
    let mutations: Vec<Mutation> = drafts
        .into_iter()
        .map(|draft| {
            Mutation::Append(Slip {
                id: SlipId::new_v7(),
                text: draft.text,
                done: false,
                created_at: draft.occurred_at,
                done_at: None,
                archived_at: None,
            })
        })
        .collect();
    assert_eq!(mutations.len(), 3, "空行应被丢弃");

    let ack = dispatcher.apply(String::from("r1"), mutations).await;
    assert_eq!(ack.result, AckResult::Applied, "{ack:?}");

    let snapshot = dispatcher.get_snapshot().await.unwrap();
    let texts: Vec<&str> = snapshot
        .slips
        .iter()
        .map(|slip| slip.text.as_str())
        .collect();
    assert_eq!(texts, vec!["第一件事", "第二件事", "第三件事"]);

    let raw = std::fs::read_to_string(&inbox).unwrap();
    assert_eq!(raw.lines().count(), 3, "三条记录应各占一行: {raw:?}");
    dispatcher.shutdown().await;
}

#[tokio::test(flavor = "current_thread")]
async fn domain_rejection_does_not_touch_the_file() {
    let dir = tempfile::tempdir().expect("temp vault");
    let inbox = dir.path().join("inbox.md");
    let store = Arc::new(MarkdownStore::new(&inbox));
    let prefs = Arc::new(JsonPrefsStore::new(dir.path().join("prefs.json")));
    let presenter = Presenter::with_view(store.snapshot(), Default::default());
    let dispatcher = Dispatcher::spawn(store, prefs, presenter, 8);

    let before = std::fs::read(&inbox).unwrap_or_default();
    // 空白正文在领域层就该被拒。
    let ack = dispatcher
        .apply(String::from("r1"), vec![Mutation::Append(slip("   "))])
        .await;

    assert_eq!(ack.result, AckResult::Rejected, "{ack:?}");
    assert_eq!(ack.error.unwrap().code, "domain");
    assert_eq!(std::fs::read(&inbox).unwrap_or_default(), before);
    dispatcher.shutdown().await;
}
