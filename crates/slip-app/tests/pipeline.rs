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

/// 临时 vault + 真实 store + 真实调度器。
fn dispatcher_for(inbox: &std::path::Path) -> Dispatcher {
    let store = Arc::new(MarkdownStore::new(inbox));
    let presenter = Presenter::with_view(store.snapshot(), Default::default());
    Dispatcher::spawn(store, presenter, 8)
}

/// 提交单条 patch；ACK 原样交回，由测试自己断言。
async fn apply_patch(
    dispatcher: &Dispatcher,
    request_id: &str,
    id: SlipId,
    patch: Patch,
) -> slip_app::Ack {
    dispatcher
        .apply(request_id.to_owned(), vec![Mutation::Patch { id, patch }])
        .await
}

/// 每次成功写入都会留下 `<basename>.bak-<unix_ms>[-n]` 备份。
/// 取字典序最大的那个（时间戳定长，同毫秒追 `-n`）。
fn latest_backup_name(dir: &std::path::Path) -> String {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .expect("read vault dir")
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name.contains(".bak-"))
        .collect();
    names.sort();
    names.pop().expect("一次成功写入之后应当存在备份")
}

/// 从 marker 里删掉 `;done_at=<time>` 字段，其余部分保持合法。
fn strip_done_at(line: &str) -> String {
    let Some(start) = line.find(";done_at=") else {
        return line.to_owned();
    };
    let rest = &line[start..];
    // 从 `;done_at=` 的值之后开始找下一个分隔符；否则会立刻匹配到字段自己的前导 `;`。
    let value_start = ";done_at=".len();
    let end = rest[value_start..]
        .find(';')
        .map(|offset| offset + value_start)
        .or_else(|| rest.find(" -->"))
        .expect("done_at 字段应有结束位置");
    let mut stripped = String::with_capacity(line.len());
    stripped.push_str(&line[..start]);
    stripped.push_str(&rest[end..]);
    stripped
}

/// 创建 → 完成 → 归档。这是"整体替换类操作"测试共用的前置状态。
async fn seed_archived(dispatcher: &Dispatcher, text: &str) -> Slip {
    let created = slip(text);
    let ack = dispatcher
        .apply(
            String::from("prep-append"),
            vec![Mutation::Append(created.clone())],
        )
        .await;
    assert_eq!(ack.result, AckResult::Applied, "{ack:?}");

    let ack = apply_patch(
        dispatcher,
        "prep-done",
        created.id,
        Patch::SetDone {
            done: true,
            at: OffsetDateTime::now_utc(),
        },
    )
    .await;
    assert_eq!(ack.result, AckResult::Applied, "{ack:?}");

    let ack = apply_patch(
        dispatcher,
        "prep-archive",
        created.id,
        Patch::Archive {
            at: OffsetDateTime::now_utc(),
        },
    )
    .await;
    assert_eq!(ack.result, AckResult::Applied, "{ack:?}");

    created
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

/// 恢复失败不是"内容被整体替换"，撤销历史必须原样保留。
///
/// `restore_backup` 失败时文件根本没动，栈里那一步仍然描述当前文档。
/// 清掉它等于凭空吃掉用户的一次撤销；更糟的是用户会看到"恢复失败"之后
/// Ctrl+Z 也没反应，误以为正文已经丢了。
#[tokio::test(flavor = "current_thread")]
async fn failed_restore_keeps_the_undo_stack() {
    let dir = tempfile::tempdir().expect("temp vault");
    let inbox = dir.path().join("inbox.md");
    let dispatcher = dispatcher_for(&inbox);

    let created = slip("编辑前正文");
    let ack = dispatcher
        .apply(String::from("r1"), vec![Mutation::Append(created.clone())])
        .await;
    assert_eq!(ack.result, AckResult::Applied, "{ack:?}");

    let ack = apply_patch(
        &dispatcher,
        "r2",
        created.id,
        Patch::SetText(String::from("编辑后正文")),
    )
    .await;
    assert_eq!(ack.result, AckResult::Applied, "{ack:?}");

    // `parse_backup_name` 不认的名字 → NotFound；文件不动，栈也不该动。
    let ack = dispatcher
        .restore_backup(String::from("r3"), String::from("nope.md"))
        .await;
    assert_ne!(
        ack.result,
        AckResult::Applied,
        "不存在的备份名不能被当作恢复成功: {ack:?}"
    );

    let ack = dispatcher.undo(String::from("r4")).await;
    assert_eq!(
        ack.result,
        AckResult::Applied,
        "失败的恢复吃掉了撤销步: {ack:?}"
    );

    let snapshot = dispatcher.get_snapshot().await.unwrap();
    assert_eq!(snapshot.slips[0].text, "编辑前正文");
    let raw = std::fs::read_to_string(&inbox).expect("read inbox");
    assert!(raw.contains("编辑前正文"), "undo did not restore: {raw:?}");
    assert!(!raw.contains("编辑后正文"), "stale text on disk: {raw:?}");

    dispatcher.shutdown().await;
}

/// 恢复成功 = 内容被整体替换，撤销栈必须清空。
///
/// 栈里记录的是"替换前那份文档"的逆操作；恢复之后它指向的条目/正文已经不属于
/// 当前文档，重放只会得到一条自相矛盾的错误。
#[tokio::test(flavor = "current_thread")]
async fn successful_restore_clears_the_undo_stack() {
    let dir = tempfile::tempdir().expect("temp vault");
    let inbox = dir.path().join("inbox.md");
    let dispatcher = dispatcher_for(&inbox);

    let created = slip("恢复前的原始正文");
    let ack = dispatcher
        .apply(String::from("r1"), vec![Mutation::Append(created.clone())])
        .await;
    assert_eq!(ack.result, AckResult::Applied, "{ack:?}");

    // 这次编辑进撤销栈。
    let ack = apply_patch(
        &dispatcher,
        "r2",
        created.id,
        Patch::SetText(String::from("恢复前的新正文")),
    )
    .await;
    assert_eq!(ack.result, AckResult::Applied, "{ack:?}");

    // 每次成功写入都留了备份，取最新的一份（即编辑前的原文）。
    let name = latest_backup_name(dir.path());
    let ack = dispatcher.restore_backup(String::from("r3"), name).await;
    assert_eq!(ack.result, AckResult::Applied, "恢复应当成功: {ack:?}");

    let ack = dispatcher.undo(String::from("r4")).await;
    assert_eq!(ack.result, AckResult::Rejected, "{ack:?}");
    assert_eq!(ack.error.expect("error").code, "undo");

    dispatcher.shutdown().await;
}

/// purge 撞上外部修改时必须是 `conflict`，不能降级成笼统的 `rejected`。
///
/// 前端按 `result`/`code` 决定提示与主操作；把冲突说成"被拒绝"会让用户
/// 永远等不到"重新载入"这个正确的下一步。
#[tokio::test(flavor = "current_thread")]
async fn purge_conflict_maps_to_conflict_result() {
    let dir = tempfile::tempdir().expect("temp vault");
    let inbox = dir.path().join("inbox.md");
    let dispatcher = dispatcher_for(&inbox);

    let created = seed_archived(&dispatcher, "归档后被外部改过的一条").await;

    // 外部追加一行普通文本：内容 hash 变了，但文档依然可解析。
    let mut raw = std::fs::read_to_string(&inbox).expect("read inbox");
    raw.push_str("外部追加的一行普通文本\n");
    std::fs::write(&inbox, raw).expect("external edit");

    let ack = dispatcher.purge(String::from("r1"), created.id).await;
    assert_eq!(
        ack.result,
        AckResult::Conflict,
        "purge 的冲突必须映射成 Conflict: {ack:?}"
    );
    assert_eq!(
        ack.error.as_ref().map(|error| error.code.as_str()),
        Some("conflict")
    );

    dispatcher.shutdown().await;
}

/// purge 成功 = 那一行真的没了，且撤销栈里指向它的记录必须一并清空。
///
/// 不清的话，归档那一步的逆操作会指向一条已不存在的条目，之后每次撤销
/// 都撞同一堵墙（与 reload / isolate 同类）。
#[tokio::test(flavor = "current_thread")]
async fn purge_removes_the_line_and_clears_the_undo_stack() {
    let dir = tempfile::tempdir().expect("temp vault");
    let inbox = dir.path().join("inbox.md");
    let dispatcher = dispatcher_for(&inbox);

    let created = seed_archived(&dispatcher, "会被彻底移除的一条").await;
    let id = created.id.to_canonical_string();

    // 此刻撤销栈里有归档那一步；purge 成功必须把它和其他旧记录一起清掉。
    let ack = dispatcher.purge(String::from("r1"), created.id).await;
    assert_eq!(ack.result, AckResult::Applied, "{ack:?}");

    let raw = std::fs::read_to_string(&inbox).expect("read inbox");
    assert!(!raw.contains(&id), "purge 之后磁盘上不该再有该 id: {raw:?}");
    let snapshot = dispatcher.get_snapshot().await.unwrap();
    assert!(
        snapshot.slips.is_empty(),
        "snapshot 应当少一条: {snapshot:?}"
    );

    let ack = dispatcher.undo(String::from("r2")).await;
    assert_eq!(ack.result, AckResult::Rejected, "{ack:?}");
    assert_eq!(ack.error.expect("error").code, "undo");

    dispatcher.shutdown().await;
}

/// Noop 也是 `Applied`，但它不是一个"可撤销的步骤"。
///
/// 把 Noop 压进栈会让 Ctrl+Z 白白消耗一次：用户看到"已撤销"，正文却停在原处。
#[tokio::test(flavor = "current_thread")]
async fn noop_apply_does_not_consume_an_undo_step() {
    let dir = tempfile::tempdir().expect("temp vault");
    let inbox = dir.path().join("inbox.md");
    let dispatcher = dispatcher_for(&inbox);

    let created = slip("第一版正文");
    let ack = dispatcher
        .apply(String::from("r1"), vec![Mutation::Append(created.clone())])
        .await;
    assert_eq!(ack.result, AckResult::Applied, "{ack:?}");

    let ack = apply_patch(
        &dispatcher,
        "r2",
        created.id,
        Patch::SetText(String::from("第二版正文")),
    )
    .await;
    assert_eq!(ack.result, AckResult::Applied, "{ack:?}");

    // 同一份正文再提交一次：成功，但 content_rev 不前进。
    let before_noop = dispatcher.get_snapshot().await.unwrap().content_rev;
    let ack = apply_patch(
        &dispatcher,
        "r3",
        created.id,
        Patch::SetText(String::from("第二版正文")),
    )
    .await;
    assert_eq!(ack.result, AckResult::Applied, "noop 仍然是成功: {ack:?}");
    let after_noop = dispatcher.get_snapshot().await.unwrap().content_rev;
    assert_eq!(
        after_noop, before_noop,
        "重复提交同一正文应当是 Noop，content_rev 不该前进"
    );

    // 只撤一次，必须回到第一版——Noop 不该占掉这一步。
    let ack = dispatcher.undo(String::from("r4")).await;
    assert_eq!(ack.result, AckResult::Applied, "{ack:?}");

    let snapshot = dispatcher.get_snapshot().await.unwrap();
    assert_eq!(
        snapshot.slips[0].text, "第一版正文",
        "Noop 消耗了一次撤销步"
    );
    let raw = std::fs::read_to_string(&inbox).expect("read inbox");
    assert!(raw.contains("第一版正文"), "undo did not restore: {raw:?}");
    assert!(!raw.contains("第二版正文"), "stale text on disk: {raw:?}");

    dispatcher.shutdown().await;
}

/// 在归档抽屉里取消完成，再撤销：完成与归档必须一起回来。
///
/// 只发 `SetDone { done: true }` 会把 `archived_at` 丢掉——条目回到默认列表，
/// 用户刚在抽屉里做的操作被撤销成了"另一个状态"。
#[tokio::test(flavor = "current_thread")]
async fn unchecking_an_archived_slip_can_be_undone() {
    let dir = tempfile::tempdir().expect("temp vault");
    let inbox = dir.path().join("inbox.md");
    let dispatcher = dispatcher_for(&inbox);

    let created = seed_archived(&dispatcher, "归档抽屉里取消完成的一条").await;

    // 归档抽屉里的"取消完成"：`SetDone { done: false }` 同时清掉完成时间与归档墓碑。
    let ack = apply_patch(
        &dispatcher,
        "r1",
        created.id,
        Patch::SetDone {
            done: false,
            at: OffsetDateTime::now_utc(),
        },
    )
    .await;
    assert_eq!(ack.result, AckResult::Applied, "{ack:?}");

    let raw = std::fs::read_to_string(&inbox).expect("read inbox");
    assert!(
        !raw.contains(";archived_at="),
        "取消完成应当清掉墓碑: {raw:?}"
    );

    let ack = dispatcher.undo(String::from("r2")).await;
    assert_eq!(ack.result, AckResult::Applied, "{ack:?}");

    let snapshot = dispatcher.get_snapshot().await.unwrap();
    let slip = &snapshot.slips[0];
    assert!(slip.done, "撤销后必须恢复完成: {slip:?}");
    assert!(slip.archived, "撤销后必须恢复归档: {slip:?}");
    let raw = std::fs::read_to_string(&inbox).expect("read inbox");
    assert!(
        raw.contains(";archived_at="),
        "磁盘上必须重新出现 archived_at: {raw:?}"
    );
    assert!(raw.contains("- [x] "), "复选框必须回到已完成: {raw:?}");

    dispatcher.shutdown().await;
}

/// `done = true` 但缺 `done_at` 也能被外部改出来；撤销不能把它当成"未完成"。
///
/// 逆操作若只看 `done_at` 是否存在，就会生成 `SetDone { done: false }`，
/// 撤销变成了一次 Noop——用户按了 Ctrl+Z，勾选却没有回来。
#[tokio::test(flavor = "current_thread")]
async fn done_without_done_at_is_restored() {
    let dir = tempfile::tempdir().expect("temp vault");
    let inbox = dir.path().join("inbox.md");
    let dispatcher = dispatcher_for(&inbox);

    let created = slip("完成时间被手工删掉的一条");
    let ack = dispatcher
        .apply(String::from("r1"), vec![Mutation::Append(created.clone())])
        .await;
    assert_eq!(ack.result, AckResult::Applied, "{ack:?}");

    let ack = apply_patch(
        &dispatcher,
        "r2",
        created.id,
        Patch::SetDone {
            done: true,
            at: OffsetDateTime::now_utc(),
        },
    )
    .await;
    assert_eq!(ack.result, AckResult::Applied, "{ack:?}");

    // 手工删掉该行的 `;done_at=...` 字段，保留 marker 其余部分合法。
    let id = created.id.to_canonical_string();
    let raw = std::fs::read_to_string(&inbox).expect("read inbox");
    assert!(
        raw.contains(";done_at="),
        "前置条件：应已写入完成时间: {raw:?}"
    );
    let mut rewritten = String::new();
    for line in raw.lines() {
        if line.contains(&id) {
            rewritten.push_str(&strip_done_at(line));
        } else {
            rewritten.push_str(line);
        }
        rewritten.push('\n');
    }
    assert!(
        !rewritten.contains(";done_at="),
        "手工删字段失败: {rewritten:?}"
    );
    assert!(
        rewritten.contains(&id) && rewritten.contains(" -->"),
        "删字段之后 marker 仍必须合法: {rewritten:?}"
    );
    std::fs::write(&inbox, &rewritten).expect("rewrite inbox");

    let ack = dispatcher.reload(String::from("r3")).await;
    assert_eq!(ack.result, AckResult::Applied, "{ack:?}");
    let snapshot = dispatcher.get_snapshot().await.unwrap();
    assert!(snapshot.slips[0].done);
    assert_eq!(
        snapshot.slips[0].done_at, None,
        "前置条件：完成时间确实缺了"
    );

    // 取消完成，再撤销：done 必须回来（即使没有 done_at 可恢复）。
    let ack = apply_patch(
        &dispatcher,
        "r4",
        created.id,
        Patch::SetDone {
            done: false,
            at: OffsetDateTime::now_utc(),
        },
    )
    .await;
    assert_eq!(ack.result, AckResult::Applied, "{ack:?}");

    let ack = dispatcher.undo(String::from("r5")).await;
    assert_eq!(ack.result, AckResult::Applied, "撤销没有生效: {ack:?}");

    let snapshot = dispatcher.get_snapshot().await.unwrap();
    assert!(
        snapshot.slips[0].done,
        "撤销必须恢复完成状态: {:?}",
        snapshot.slips[0]
    );

    dispatcher.shutdown().await;
}
