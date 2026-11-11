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
use time::OffsetDateTime;

fn slip(text: &str) -> Slip {
    Slip {
        id: SlipId::new_v7(),
        text: text.to_owned(),
        done: false,
        created_at: OffsetDateTime::now_utc(),
        done_at: None,
        archived_at: None,
        locked: false,
        pinned: false,
    }
}

#[tokio::test(flavor = "current_thread")]
async fn capture_writes_through_to_disk_and_back_to_the_snapshot() {
    let dir = tempfile::tempdir().expect("temp vault");
    let inbox = dir.path().join("inbox.md");

    let store = Arc::new(MarkdownStore::new(&inbox));
    let presenter = Presenter::with_view(store.snapshot(), Default::default());
    let dispatcher = Dispatcher::spawn(store, presenter, 8);

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
        let presenter = Presenter::with_view(store.snapshot(), Default::default());
        let dispatcher = Dispatcher::spawn(store, presenter, 8);
        let ack = dispatcher
            .apply(String::from("r1"), vec![Mutation::Append(slip("一次写入"))])
            .await;
        assert_eq!(ack.result, AckResult::Applied);
        dispatcher.shutdown().await;
    }

    // 模拟重新启动：全新的 store 读同一个文件。
    let store = Arc::new(MarkdownStore::new(&inbox));
    let presenter = Presenter::with_view(store.snapshot(), Default::default());
    let dispatcher = Dispatcher::spawn(store, presenter, 8);

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
    let presenter = Presenter::with_view(store.snapshot(), Default::default());
    let dispatcher = Dispatcher::spawn(store, presenter, 8);

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
                locked: false,
                pinned: false,
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
    let presenter = Presenter::with_view(store.snapshot(), Default::default());
    let dispatcher = Dispatcher::spawn(store, presenter, 8);

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

/// 编辑 → 撤销 → 锁定 → 拒绝改写，整条链路都在真实磁盘上跑一遍。
#[tokio::test(flavor = "current_thread")]
async fn edit_undo_and_lock_round_trip_through_disk() {
    let dir = tempfile::tempdir().expect("temp vault");
    let inbox = dir.path().join("inbox.md");

    let store = Arc::new(MarkdownStore::new(&inbox));
    let presenter = Presenter::with_view(store.snapshot(), Default::default());
    let dispatcher = Dispatcher::spawn(store, presenter, 8);

    let created = slip("原始内容");
    let ack = dispatcher
        .apply(String::from("r1"), vec![Mutation::Append(created.clone())])
        .await;
    assert_eq!(ack.result, AckResult::Applied, "{ack:?}");

    // 1. 改正文：磁盘与快照同时变。
    let ack = dispatcher
        .apply(
            String::from("r2"),
            vec![Mutation::Patch {
                id: created.id,
                patch: Patch::SetText(String::from("改过的内容")),
            }],
        )
        .await;
    assert_eq!(ack.result, AckResult::Applied, "{ack:?}");
    assert!(
        std::fs::read_to_string(&inbox)
            .unwrap()
            .contains("改过的内容")
    );

    // 2. 撤销把正文退回去，行本身还在。
    let ack = dispatcher.undo(String::from("r3")).await;
    assert_eq!(ack.result, AckResult::Applied, "{ack:?}");
    let raw = std::fs::read_to_string(&inbox).expect("read inbox");
    assert!(raw.contains("原始内容"), "undo did not restore: {raw:?}");
    assert!(!raw.contains("改过的内容"), "stale text on disk: {raw:?}");
    assert_eq!(
        dispatcher.get_snapshot().await.unwrap().slips[0].text,
        "原始内容"
    );

    // 3. 栈空了之后，撤销必须明确拒绝，而不是假装成功。
    let ack = dispatcher.undo(String::from("r4")).await;
    assert_eq!(ack.result, AckResult::Rejected, "{ack:?}");
    assert_eq!(ack.error.unwrap().code, "undo");

    // 4. 锁定落到 marker 上，之后正文改不动，但完成照常。
    let ack = dispatcher
        .apply(
            String::from("r5"),
            vec![Mutation::Patch {
                id: created.id,
                patch: Patch::SetLocked(true),
            }],
        )
        .await;
    assert_eq!(ack.result, AckResult::Applied, "{ack:?}");
    assert!(
        std::fs::read_to_string(&inbox)
            .unwrap()
            .contains(";locked=1")
    );
    assert!(dispatcher.get_snapshot().await.unwrap().slips[0].locked);

    let ack = dispatcher
        .apply(
            String::from("r6"),
            vec![Mutation::Patch {
                id: created.id,
                patch: Patch::SetText(String::from("偷偷改掉")),
            }],
        )
        .await;
    assert_eq!(ack.result, AckResult::Rejected, "{ack:?}");
    assert!(
        !std::fs::read_to_string(&inbox)
            .unwrap()
            .contains("偷偷改掉")
    );

    let ack = dispatcher
        .apply(
            String::from("r7"),
            vec![Mutation::Patch {
                id: created.id,
                patch: Patch::SetDone {
                    done: true,
                    at: OffsetDateTime::now_utc(),
                },
            }],
        )
        .await;
    assert_eq!(
        ack.result,
        AckResult::Applied,
        "lock blocked completion: {ack:?}"
    );

    dispatcher.shutdown().await;
}

/// 归档已完成条目 → 落盘并隐藏 → 撤销归档恢复可见且保留完成时间。
#[tokio::test(flavor = "current_thread")]
async fn archive_and_undo_preserve_done_state_through_disk() {
    let dir = tempfile::tempdir().expect("temp vault");
    let inbox = dir.path().join("inbox.md");

    let store = Arc::new(MarkdownStore::new(&inbox));
    let presenter = Presenter::with_view(store.snapshot(), Default::default());
    let dispatcher = Dispatcher::spawn(store, presenter, 8);

    let first = slip("交水电费");
    let second = slip("寄快递");
    let ack = dispatcher
        .apply(
            String::from("r1"),
            vec![
                Mutation::Append(first.clone()),
                Mutation::Append(second.clone()),
            ],
        )
        .await;
    assert_eq!(ack.result, AckResult::Applied);

    let done_at = OffsetDateTime::now_utc();
    let ack = dispatcher
        .apply(
            String::from("r2"),
            vec![
                Mutation::Patch {
                    id: first.id,
                    patch: Patch::SetDone {
                        done: true,
                        at: done_at,
                    },
                },
                Mutation::Patch {
                    id: second.id,
                    patch: Patch::SetDone {
                        done: true,
                        at: done_at,
                    },
                },
            ],
        )
        .await;
    assert_eq!(ack.result, AckResult::Applied);

    // 批量归档两条已完成纸条。
    let archived_at = OffsetDateTime::now_utc();
    let ack = dispatcher
        .apply(
            String::from("r3"),
            vec![
                Mutation::Patch {
                    id: first.id,
                    patch: Patch::Archive { at: archived_at },
                },
                Mutation::Patch {
                    id: second.id,
                    patch: Patch::Archive { at: archived_at },
                },
            ],
        )
        .await;
    assert_eq!(ack.result, AckResult::Applied);

    let raw = std::fs::read_to_string(&inbox).expect("read inbox");
    assert!(
        raw.contains(";archived_at="),
        "archived_at missing: {raw:?}"
    );
    let snapshot = dispatcher.get_snapshot().await.unwrap();
    assert!(snapshot.slips.iter().all(|s| s.archived));

    // 撤销一次即可把整批归档恢复为"已完成但未归档"，且保留原完成时间。
    let ack = dispatcher.undo(String::from("r4")).await;
    assert_eq!(ack.result, AckResult::Applied);

    let raw = std::fs::read_to_string(&inbox).expect("read inbox");
    assert!(
        !raw.contains(";archived_at="),
        "archived_at should be removed after undo: {raw:?}"
    );
    assert!(
        raw.contains(";done_at="),
        "done_at must survive unarchive: {raw:?}"
    );
    let snapshot = dispatcher.get_snapshot().await.unwrap();
    assert!(snapshot.slips.iter().all(|s| s.done && !s.archived));

    dispatcher.shutdown().await;
}

/// 重新载入 = 采纳磁盘上那一份，撤销栈必须一并清空。
///
/// 不清的话，栈里的逆操作描述的是**另一份文档**：实测外部替换文件并重载后按 Ctrl+Z，
/// 会得到 "reload is required before writing again" 这种自相矛盾的提示，而失败又会
/// 把这一步放回栈里，于是之后每次撤销都撞同一堵墙。
#[tokio::test(flavor = "current_thread")]
async fn reload_clears_the_undo_stack() {
    let dir = tempfile::tempdir().expect("temp vault");
    let inbox = dir.path().join("inbox.md");
    let store = Arc::new(MarkdownStore::new(&inbox));
    let presenter = Presenter::with_view(store.snapshot(), Default::default());
    let dispatcher = Dispatcher::spawn(store, presenter, 8);

    let created = slip("会被外部删掉的一条");
    let ack = dispatcher
        .apply(String::from("r1"), vec![Mutation::Append(created.clone())])
        .await;
    assert_eq!(ack.result, AckResult::Applied);

    // 勾选完成：这是可撤销的操作，会进撤销栈。
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
    assert_eq!(ack.result, AckResult::Applied);

    // 外部替换成另一份内容，再重新载入。
    std::fs::write(
        &inbox,
        "- [ ] 外部替换后的一条 <!-- slip:v1;id=01a1c7e0-0000-7000-8000-0000000000f9;created=2026-11-10T00:00:00Z -->\n",
    )
    .expect("replace inbox");
    let ack = dispatcher.reload(String::from("r3")).await;
    assert_eq!(ack.result, AckResult::Applied);

    // 撤销栈已清空：明确说"没有可撤销的操作"，而不是撞一个看不懂的错误。
    let ack = dispatcher.undo(String::from("r4")).await;
    assert_eq!(ack.result, AckResult::Rejected);
    assert_eq!(ack.error.expect("error").code, "undo");

    dispatcher.shutdown().await;
}

/// 隔离重建 = 整库换成空文档，撤销栈必须一并清空。
///
/// 触发链：健康态下产生一次可撤销操作 → 外部把文件写坏 → 重新载入失败（降级）
/// → 隔离并重建 → 此时撤销栈若还留着旧记录，重放会撞上不存在的条目。
#[tokio::test(flavor = "current_thread")]
async fn isolate_and_reset_clears_the_undo_stack() {
    let dir = tempfile::tempdir().expect("temp vault");
    let inbox = dir.path().join("inbox.md");
    let store = Arc::new(MarkdownStore::new(&inbox));
    let presenter = Presenter::with_view(store.snapshot(), Default::default());
    let dispatcher = Dispatcher::spawn(store, presenter, 8);

    let created = slip("隔离之前的一条");
    let ack = dispatcher
        .apply(String::from("r1"), vec![Mutation::Append(created.clone())])
        .await;
    assert_eq!(ack.result, AckResult::Applied);

    // 勾选完成：可撤销，进撤销栈。
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
    assert_eq!(ack.result, AckResult::Applied);

    // 外部写坏：marker 里的 id 不是合法 UUID，解析出问题 → 整库不可写。
    std::fs::write(
        &inbox,
        "- [ ] 坏的 <!-- slip:v1;id=not-a-uuid;created=not-a-time -->\n",
    )
    .expect("corrupt inbox");

    // 重新载入失败 → 降级，此时才允许"隔离并重建"。
    let ack = dispatcher.reload(String::from("r3")).await;
    assert_ne!(ack.result, AckResult::Applied, "坏文件不该被接纳");

    let snapshot = dispatcher.get_snapshot().await.unwrap();
    assert!(!snapshot.health.writable, "坏文件后必须降级");

    let ack = dispatcher.isolate_and_reset(String::from("r4")).await;
    assert_eq!(ack.result, AckResult::Applied, "降级态下应当允许隔离重建");

    // 撤销栈已清空：给出"没有可撤销的操作"，而不是一条看不懂的错误。
    let ack = dispatcher.undo(String::from("r5")).await;
    assert_eq!(ack.result, AckResult::Rejected);
    assert_eq!(ack.error.expect("error").code, "undo");

    dispatcher.shutdown().await;
}
