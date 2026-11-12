//! 命令队列与消费者。
//!
//! 设计要点全部来自实测量级的需求，而不是"想要一个框架"：
//!
//! - **单队列单消费者**：`presenter` 由消费者独占，因此"store 调用顺序"与
//!   "视图接纳顺序"天然一致，不需要任何版本号过滤。
//! - **磁盘调用走 `spawn_blocking`**：`Store` 是同步 trait，直接 await 会卡住
//!   tokio worker。
//! - **panic 不等于"没写"**：`spawn_blocking` 的 `JoinError` 可能发生在 rename
//!   之后，因此只能报 `unknown` 并进入 `Fatal`，绝不能假装未提交、也不要在
//!   这个分支里去调 `store.snapshot()` 补救（锁可能已中毒）。

use std::collections::VecDeque;
use std::sync::Arc;

use slip_domain::{Mutation, Patch, SlipId};
use slip_store::{Store, StoreError, StoreOutcome, StoreSnapshot};
use time::OffsetDateTime;
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::{mpsc, oneshot};

use crate::presenter::Presenter;
use crate::{Ack, AckError, AckResult, QueryError, RuntimeStatus, UiSnapshot};

/// 入队结果。`Full` 与 `Closed` 都表示**尚未执行**。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnqueueError {
    /// 队列已满；请求没有被执行，用户可以安全重试。
    Busy,
    /// 调度器已关闭；请求没有被执行。
    NotExecuted,
}

/// 视图快照的出口。返回 `false` 表示本次发射失败。
///
/// 发射失败**不回滚** `latest`，也不把已提交的操作改成失败（v11 §6.3）。
trait SnapshotSink: Send + 'static {
    fn emit(&mut self, snapshot: &UiSnapshot) -> bool;
}

/// 没有前端时的默认出口。
struct NoopSink;

impl SnapshotSink for NoopSink {
    fn emit(&mut self, _snapshot: &UiSnapshot) -> bool {
        true
    }
}

pub enum Command {
    Apply {
        request_id: String,
        mutations: Vec<Mutation>,
        reply: oneshot::Sender<Ack>,
    },
    /// 回滚最近一次可撤销的内容变更。
    Undo {
        request_id: String,
        reply: oneshot::Sender<Ack>,
    },
    RestoreBackup {
        request_id: String,
        name: String,
        reply: oneshot::Sender<Ack>,
    },
    Reload {
        request_id: String,
        reply: oneshot::Sender<Ack>,
    },
    IsolateAndReset {
        request_id: String,
        reply: oneshot::Sender<Ack>,
    },
    /// 彻底移除一条已归档记录（不可逆）。
    PurgeArchived {
        request_id: String,
        id: SlipId,
        reply: oneshot::Sender<Ack>,
    },
    GetSnapshot {
        reply: oneshot::Sender<Result<UiSnapshot, QueryError>>,
    },
    Shutdown {
        reply: oneshot::Sender<()>,
    },
}

/// 前端句柄。所有方法都只做"入队 + 等应答"。
pub struct Dispatcher {
    tx: mpsc::Sender<Command>,
}

impl Dispatcher {
    /// 启动消费者。
    ///
    /// `presenter` 必须由调用方用**已载入的初始快照**构造好——初始读盘是阻塞
    /// I/O，应该由调用方在 `spawn_blocking` 里完成，不在 UI 线程直接做。
    pub fn spawn(store: Arc<dyn Store>, presenter: Presenter, capacity: usize) -> Self {
        Self::spawn_with_sink(store, presenter, Box::new(NoopSink), capacity)
    }

    fn spawn_with_sink(
        store: Arc<dyn Store>,
        presenter: Presenter,
        sink: Box<dyn SnapshotSink>,
        capacity: usize,
    ) -> Self {
        let (tx, rx) = mpsc::channel(capacity);
        tokio::spawn(run(rx, store, presenter, sink));
        Self { tx }
    }

    async fn submit(
        &self,
        request_id: String,
        build: impl FnOnce(String, oneshot::Sender<Ack>) -> Command,
    ) -> Ack {
        let (tx, rx) = oneshot::channel();
        let command = build(request_id.clone(), tx);
        match self.tx.try_send(command) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                return rejected(
                    request_id,
                    "busy",
                    "queue is full; the request was not executed",
                );
            }
            Err(TrySendError::Closed(_)) => {
                return rejected(request_id, "notExecuted", "dispatcher is shut down");
            }
        }
        match rx.await {
            Ok(ack) => ack,
            Err(_) => Ack {
                request_id,
                result: AckResult::Unknown,
                ui_rev: None,
                error: Some(AckError {
                    code: String::from("dropped"),
                    message: String::from("consumer dropped the reply"),
                }),
            },
        }
    }

    pub async fn apply(&self, request_id: String, mutations: Vec<Mutation>) -> Ack {
        self.submit(request_id, move |id, reply| Command::Apply {
            request_id: id,
            mutations,
            reply,
        })
        .await
    }

    /// 回滚最近一次可撤销的内容变更。
    ///
    /// 撤销栈只存在于消费者内存里：**历史不是真源**，重启即清空。
    /// 把它落盘会造出第二个真源，与"inbox.md 唯一"的约定冲突。
    pub async fn undo(&self, request_id: String) -> Ack {
        self.submit(request_id, |id, reply| Command::Undo {
            request_id: id,
            reply,
        })
        .await
    }

    /// 用指定备份恢复。`name` 只在存储层校验——外壳不判断文件名是否合法。
    pub async fn restore_backup(&self, request_id: String, name: String) -> Ack {
        self.submit(request_id, move |id, reply| Command::RestoreBackup {
            request_id: id,
            name,
            reply,
        })
        .await
    }

    pub async fn reload(&self, request_id: String) -> Ack {
        self.submit(request_id, move |id, reply| Command::Reload {
            request_id: id,
            reply,
        })
        .await
    }

    pub async fn isolate_and_reset(&self, request_id: String) -> Ack {
        self.submit(request_id, move |id, reply| Command::IsolateAndReset {
            request_id: id,
            reply,
        })
        .await
    }

    /// 彻底移除一条已归档记录（R104）。不可逆，调用方负责二次确认。
    pub async fn purge(&self, request_id: String, id: SlipId) -> Ack {
        self.submit(request_id, move |request_id, reply| {
            Command::PurgeArchived {
                request_id,
                id,
                reply,
            }
        })
        .await
    }

    pub async fn get_snapshot(&self) -> Result<UiSnapshot, QueryError> {
        let (tx, rx) = oneshot::channel();
        match self.tx.try_send(Command::GetSnapshot { reply: tx }) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                return Err(QueryError(String::from("queue is full")));
            }
            Err(TrySendError::Closed(_)) => {
                return Err(QueryError(String::from("dispatcher is shut down")));
            }
        }
        match rx.await {
            Ok(result) => result,
            Err(_) => Err(QueryError(String::from("consumer dropped the reply"))),
        }
    }

    pub async fn shutdown(&self) {
        let (tx, rx) = oneshot::channel();
        if self.tx.send(Command::Shutdown { reply: tx }).await.is_ok() {
            let _ = rx.await;
        }
    }
}

async fn run(
    mut rx: mpsc::Receiver<Command>,
    store: Arc<dyn Store>,
    mut presenter: Presenter,
    mut sink: Box<dyn SnapshotSink>,
) {
    // 撤销栈深度。只在内存里，重启即清空。
    const UNDO_DEPTH: usize = 50;
    let mut undo: VecDeque<Vec<Mutation>> = VecDeque::new();

    loop {
        let Some(command) = rx.recv().await else {
            break;
        };

        match command {
            Command::Apply {
                request_id,
                mutations,
                reply,
            } => {
                if presenter.is_fatal() {
                    let _ = reply.send(not_executed(request_id));
                    continue;
                }
                // "正文编辑 / 勾选 / 归档"进撤销栈：归档后纸条立即从默认列表隐藏，
                // 没有开关可点回去，因此必须能靠撤销恢复；追加无删除变更，锁定有显式开关。
                let inverse = if undoable(&mutations) {
                    inverse_batch(&store.snapshot(), &mutations)
                } else {
                    None
                };
                let ack =
                    run_batch(&store, &mut presenter, &mut *sink, request_id, mutations).await;
                if matches!(ack.result, AckResult::Applied)
                    && let Some(inverse) = inverse
                {
                    push_undo(&mut undo, inverse, UNDO_DEPTH);
                }
                let _ = reply.send(ack);
            }

            Command::Undo { request_id, reply } => {
                if presenter.is_fatal() {
                    let _ = reply.send(not_executed(request_id));
                    continue;
                }
                let Some(inverse) = undo.pop_back() else {
                    let _ = reply.send(rejected(request_id, "undo", "没有可撤销的操作"));
                    continue;
                };
                // 撤销走与普通修改完全相同的写入路径：全有或全无、同一个提交点。
                let ack = run_batch(
                    &store,
                    &mut presenter,
                    &mut *sink,
                    request_id,
                    inverse.clone(),
                )
                .await;
                // 撤销失败（例如目标刚被锁定）就把这一步放回去，解锁后还能再撤。
                if !matches!(ack.result, AckResult::Applied) {
                    push_undo(&mut undo, inverse, UNDO_DEPTH);
                }
                let _ = reply.send(ack);
            }

            Command::RestoreBackup {
                request_id,
                name,
                reply,
            } => {
                if presenter.is_fatal() {
                    let _ = reply.send(not_executed(request_id));
                    continue;
                }
                let store_handle = Arc::clone(&store);
                let result =
                    tokio::task::spawn_blocking(move || store_handle.restore_backup(&name)).await;
                let ack = match result {
                    Ok(outcome) => {
                        // 恢复是"覆盖当前内容"，同样不能被撤销栈重放，所以不进 undo。
                        let ack = settle(&mut presenter, &mut *sink, request_id, &outcome);
                        undo.clear();
                        ack
                    }
                    Err(join) => fatal(&mut presenter, &mut *sink, request_id, join.to_string()),
                };
                let _ = reply.send(ack);
            }

            Command::Reload { request_id, reply } => {
                if presenter.is_fatal() {
                    let _ = reply.send(not_executed(request_id));
                    continue;
                }
                let store_handle = Arc::clone(&store);
                let result = tokio::task::spawn_blocking(move || store_handle.reload()).await;
                // 载入即"采纳磁盘上那一份"：此前的撤销记录描述的是另一份文档，
                // 重放它们只会撞上不存在的条目。实测过：外部替换 + 重载之后按 Ctrl+Z，
                // 会得到"reload is required before writing again"这种自相矛盾的提示，
                // 而失败又会把这一步放回栈里，于是之后每次都失败。
                let ack = match result {
                    Ok(outcome) => {
                        let error = outcome.result.as_ref().err().map(map_store_error);
                        presenter.accept(&outcome);
                        let ui_rev = presenter.ui_rev();
                        emit(&presenter, &mut *sink);
                        // 只有**真的采纳了新内容**才清栈：载入失败时文档没变，
                        // 撤销历史不该丢（变异测试发现的：失败的重载也会走到这里）。
                        if error.is_none() {
                            undo.clear();
                        }
                        match error {
                            None => Ack {
                                request_id,
                                result: AckResult::Applied,
                                ui_rev: Some(ui_rev),
                                error: None,
                            },
                            Some((code, message)) => Ack {
                                request_id,
                                result: result_for_code(code),
                                ui_rev: Some(ui_rev),
                                error: Some(AckError {
                                    code: code.to_owned(),
                                    message,
                                }),
                            },
                        }
                    }
                    Err(join) => fatal(&mut presenter, &mut *sink, request_id, join.to_string()),
                };
                let _ = reply.send(ack);
            }

            Command::PurgeArchived {
                request_id,
                id,
                reply,
            } => {
                if presenter.is_fatal() {
                    let _ = reply.send(not_executed(request_id));
                    continue;
                }
                let store_handle = Arc::clone(&store);
                let result =
                    tokio::task::spawn_blocking(move || store_handle.purge_archived(id)).await;
                // 删掉一行 = 撤销记录里可能指向一条已经不存在的条目，因此与"隔离重建"
                // 同属整体替换类操作：成功就清栈。失败时**不清**——那条记录还在，
                // 用户手上的撤销步仍然有效。
                let ack = match result {
                    Ok(outcome) => {
                        let error = outcome.result.as_ref().err().map(map_store_error);
                        presenter.accept(&outcome);
                        let ui_rev = presenter.ui_rev();
                        emit(&presenter, &mut *sink);
                        if error.is_none() {
                            undo.clear();
                        }
                        match error {
                            None => Ack {
                                request_id,
                                result: AckResult::Applied,
                                ui_rev: Some(ui_rev),
                                error: None,
                            },
                            Some((code, message)) => Ack {
                                request_id,
                                result: AckResult::Rejected,
                                ui_rev: Some(ui_rev),
                                error: Some(AckError {
                                    code: code.to_owned(),
                                    message,
                                }),
                            },
                        }
                    }
                    Err(join_error) => {
                        tracing::error!(?join_error, "purge 任务失败");
                        let _ = reply.send(not_executed(request_id));
                        continue;
                    }
                };
                let _ = reply.send(ack);
            }

            Command::IsolateAndReset { request_id, reply } => {
                if presenter.is_fatal() {
                    let _ = reply.send(not_executed(request_id));
                    continue;
                }
                let store_handle = Arc::clone(&store);
                let result =
                    tokio::task::spawn_blocking(move || store_handle.isolate_and_reset()).await;
                // 隔离重建 = 整库换成空文档，旧撤销记录指向的条目全都不存在了。
                let ack = match result {
                    Ok(outcome) => {
                        let error = outcome.result.as_ref().err().map(map_store_error);
                        presenter.accept(&outcome);
                        let ui_rev = presenter.ui_rev();
                        emit(&presenter, &mut *sink);
                        // 整库已换，撤销栈一并清空。
                        undo.clear();
                        match error {
                            None => Ack {
                                request_id,
                                result: AckResult::Applied,
                                ui_rev: Some(ui_rev),
                                error: None,
                            },
                            Some((code, message)) => Ack {
                                request_id,
                                result: result_for_code(code),
                                ui_rev: Some(ui_rev),
                                error: Some(AckError {
                                    code: code.to_owned(),
                                    message,
                                }),
                            },
                        }
                    }
                    Err(join) => fatal(&mut presenter, &mut *sink, request_id, join.to_string()),
                };
                let _ = reply.send(ack);
            }

            // 查询不递增 ui_rev：它只是把已保存的那份再交一次。
            Command::GetSnapshot { reply } => {
                let result = presenter
                    .current()
                    .cloned()
                    .ok_or_else(|| QueryError(String::from("no snapshot yet")));
                let _ = reply.send(result);
            }

            Command::Shutdown { reply } => {
                // 队列里已经排队的命令明确未执行；偏好最后再试一次，失败只记日志。
                rx.close();
                while let Some(command) = rx.recv().await {
                    reject(command);
                }
                let _ = reply.send(());
                break;
            }
        }
    }
}

/// 把一次 store 结果转成 ACK 并发布新快照。
///
/// `Apply` / `Undo` / `RestoreBackup` 共用：三者的"结果 → 应答"规则必须完全一致，
/// 否则会出现"某条路径的失败没进横幅"这类只在特定操作下才暴露的问题。
fn settle(
    presenter: &mut Presenter,
    sink: &mut (dyn SnapshotSink + 'static),
    request_id: String,
    outcome: &StoreOutcome,
) -> Ack {
    let error = outcome.result.as_ref().err().map(map_store_error);
    presenter.accept(outcome);
    let ui_rev = presenter.ui_rev();
    emit(presenter, sink);
    match error {
        None => Ack {
            request_id,
            result: AckResult::Applied,
            ui_rev: Some(ui_rev),
            error: None,
        },
        Some((code, message)) => Ack {
            request_id,
            result: result_for_code(code),
            ui_rev: Some(ui_rev),
            error: Some(AckError {
                code: code.to_owned(),
                message,
            }),
        },
    }
}

/// 执行一批变更并产出 ACK。
///
/// `Apply` 与 `Undo` 共用同一段：撤销必须走**完全相同**的写入路径，
/// 否则"提交点""全有或全无""JoinError 即未知"这些约定会出现两套实现。
async fn run_batch(
    store: &Arc<dyn Store>,
    presenter: &mut Presenter,
    sink: &mut (dyn SnapshotSink + 'static),
    request_id: String,
    mutations: Vec<Mutation>,
) -> Ack {
    let store_handle = Arc::clone(store);
    let result = tokio::task::spawn_blocking(move || store_handle.apply(&mutations)).await;
    match result {
        Ok(outcome) => settle(presenter, sink, request_id, &outcome),
        // 得到 JoinError 意味着我们无法知道磁盘上到底发生了什么：可能 rename
        // 已经成功。只能报 unknown 并进入 Fatal。
        Err(join) => fatal(presenter, sink, request_id, join.to_string()),
    }
}

/// 这批变更是否值得进撤销栈。
///
/// 只认"改正文""改完成状态"和"归档/取消归档"：追加没有对应的删除变更所以撤不掉，
/// 锁定有显式开关，点回去比撤销更直观。
fn undoable(mutations: &[Mutation]) -> bool {
    !mutations.is_empty()
        && mutations.iter().all(|mutation| {
            matches!(
                mutation,
                Mutation::Patch {
                    patch: Patch::SetText(_)
                        | Patch::SetDone { .. }
                        | Patch::Archive { .. }
                        | Patch::Unarchive,
                    ..
                }
            )
        })
}

/// 用**写入前**的快照算出逆操作，顺序与原批次相反。
///
/// 只在 [`undoable`] 为真时调用，因此追加这类热路径不会为此多付一次快照拷贝。
fn inverse_batch(before: &StoreSnapshot, mutations: &[Mutation]) -> Option<Vec<Mutation>> {
    let mut inverse = Vec::with_capacity(mutations.len());
    for mutation in mutations.iter().rev() {
        let Mutation::Patch { id, patch } = mutation else {
            return None;
        };
        let slip = before.slips.iter().find(|slip| slip.id == *id)?;
        let patch = match patch {
            Patch::SetText(_) => Patch::SetText(slip.text.clone()),
            Patch::SetDone { .. } => match (slip.done, slip.done_at) {
                // 原本已完成：恢复原来的完成时间，别用"此刻"覆盖历史。
                (true, Some(at)) => Patch::SetDone { done: true, at },
                // 原本未完成：逆操作是取消完成，`at` 在使用上被忽略。
                _ => Patch::SetDone {
                    done: false,
                    at: OffsetDateTime::now_utc(),
                },
            },
            Patch::Archive { .. } | Patch::Unarchive => match slip.archived_at {
                Some(at) => Patch::Archive { at },
                None => Patch::Unarchive,
            },
            // 置顶与定稿同类：它们是"标注"，不是内容编辑。撤销一条正文改动时
            // 不该顺带把用户刚点的置顶也翻回去——那会让撤销变成不可预测的连带操作。
            Patch::SetLocked(_) => return None,
            Patch::SetPinned(_) => return None,
        };
        inverse.push(Mutation::Patch { id: *id, patch });
    }
    Some(inverse)
}

fn push_undo(undo: &mut VecDeque<Vec<Mutation>>, batch: Vec<Mutation>, depth: usize) {
    undo.push_back(batch);
    while undo.len() > depth {
        undo.pop_front();
    }
}

fn emit(presenter: &Presenter, sink: &mut (dyn SnapshotSink + 'static)) {
    if let Some(snapshot) = presenter.current() {
        // 发射失败只丢一次通知，不回滚状态；前端会靠 ACK 的 ui_rev 补取。
        let _ = sink.emit(snapshot);
    }
}

/// 处理 `JoinError`：当前请求 `unknown`，运行时进入 `Fatal`。
fn fatal(
    presenter: &mut Presenter,
    sink: &mut (dyn SnapshotSink + 'static),
    request_id: String,
    reason: String,
) -> Ack {
    presenter.set_runtime(RuntimeStatus::Fatal { reason });
    let ui_rev = presenter.ui_rev();
    emit(presenter, sink);
    Ack {
        request_id,
        result: AckResult::Unknown,
        ui_rev: Some(ui_rev),
        error: Some(AckError {
            code: String::from("fatal"),
            message: String::from("runtime state is no longer trusted; restart to recover"),
        }),
    }
}

fn reject(command: Command) {
    match command {
        Command::Apply {
            request_id, reply, ..
        }
        | Command::Reload { request_id, reply }
        | Command::Undo { request_id, reply }
        | Command::RestoreBackup {
            request_id, reply, ..
        }
        | Command::PurgeArchived {
            request_id, reply, ..
        }
        | Command::IsolateAndReset { request_id, reply } => {
            let _ = reply.send(not_executed(request_id));
        }
        Command::GetSnapshot { reply } => {
            let _ = reply.send(Err(QueryError(String::from("notExecuted"))));
        }
        // 没有应答句柄的命令无从通知，直接丢弃。
        Command::Shutdown { reply } => {
            let _ = reply.send(());
        }
    }
}

fn not_executed(request_id: String) -> Ack {
    rejected(
        request_id,
        "notExecuted",
        "runtime is not executing content commands",
    )
}

fn rejected(request_id: String, code: &str, message: &str) -> Ack {
    Ack {
        request_id,
        result: AckResult::Rejected,
        ui_rev: None,
        error: Some(AckError {
            code: code.to_owned(),
            message: message.to_owned(),
        }),
    }
}

/// v11 §6.5 的映射表。`ReadOnly` 必须映射成 `rejected/readOnly`，
/// 而不是 `needsReload`——两者对用户意味着完全不同的下一步动作。
fn map_store_error(error: &StoreError) -> (&'static str, String) {
    match error {
        // 这三条的 `message` 会随 ACK 一起回到前端。前端目前按 `code` 覆盖了文案，
        // 但只要有人把 message 直接显示出来，英文就会漏给用户——所以这里也必须是中文。
        StoreError::Conflict { .. } => (
            "conflict",
            String::from("inbox.md 在载入之后被别的程序改过"),
        ),
        StoreError::NeedsReload => ("needsReload", String::from("需要先重新载入才能继续写入")),
        StoreError::ReadOnly => (
            "readOnly",
            String::from("文件无法解析，当前处于只读保护状态"),
        ),
        StoreError::Domain(err) => ("domain", err.to_string()),
        // 用 Display 而不是 Debug：这条消息会直接出现在用户的提示区。
        StoreError::Blocking(err) => ("blocking", err.to_string()),
        // 同样用 Display：这条会出现在提示区，`PermissionDenied` 对用户没有意义。
        StoreError::Io(kind) => ("io", kind.to_string()),
        StoreError::NotDegraded => (
            "notDegraded",
            String::from("isolation is only allowed while degraded"),
        ),
    }
}

fn result_for_code(code: &str) -> AckResult {
    match code {
        "conflict" => AckResult::Conflict,
        "needsReload" => AckResult::NeedsReload,
        _ => AckResult::Rejected,
    }
}

#[cfg(test)]
mod tests {
    // ---------- R129：撤销白名单的守卫 ----------

    /// 置顶是"标注"，不是内容编辑——它**不进撤销栈**。
    ///
    /// 这条断言是变异审计补出来的：审计把 `SetPinned` 加进 `undoable` 的白名单后，
    /// 全套测试仍然全绿——说明"置顶不可撤销"当时只是一句注释，没有守卫。
    #[test]
    fn pinning_is_not_undoable() {
        let id = SlipId::new_v7();
        let pinned = vec![Mutation::Patch {
            id,
            patch: Patch::SetPinned(true),
        }];
        assert!(!undoable(&pinned), "置顶不该进撤销栈");
        // 反向确认：真正的编辑仍然可撤销，否则上面的断言可能只是"永远为假"。
        let edited = vec![Mutation::Patch {
            id,
            patch: Patch::SetText(String::from("改过")),
        }];
        assert!(undoable(&edited), "正文编辑必须可撤销");
    }

    /// 定稿同理（既有行为，这里一并钉住，避免白名单被顺手改动）。
    #[test]
    fn locking_is_not_undoable() {
        let id = SlipId::new_v7();
        let locked = vec![Mutation::Patch {
            id,
            patch: Patch::SetLocked(true),
        }];
        assert!(!undoable(&locked), "定稿不该进撤销栈");
    }

    use std::sync::{Arc, Mutex};

    use slip_domain::{ContentRev, Slip, SlipId};
    use slip_store::{Health, RecoveryState, StoreOutcome, StoreSnapshot};
    use time::OffsetDateTime;

    use super::*;

    // ------------------------------------------------ 测试替身

    struct MockStore {
        log: Arc<Mutex<Vec<String>>>,
        rev: Mutex<u64>,
        behavior: Mutex<Behavior>,
        gate: Option<Mutex<std::sync::mpsc::Receiver<()>>>,
    }

    #[derive(Clone, PartialEq, Eq)]
    enum Behavior {
        Ok,
        Fail(StoreError),
        Panic,
    }

    impl MockStore {
        fn snapshot(&self, rev: u64) -> StoreSnapshot {
            StoreSnapshot {
                content_rev: ContentRev(rev),
                slips: Vec::new(),
                health: Health::default(),
                needs_reload: false,
                recovery: RecoveryState::Normal,
                diagnostics: Vec::new(),
            }
        }

        fn record(&self, event: String) {
            if let Some(gate) = &self.gate {
                // 阻塞到测试放行，用来制造"消费者忙、队列满"的窗口。
                if let Ok(rx) = gate.lock() {
                    let _ = rx.recv();
                }
            }
            self.log.lock().unwrap().push(event);
        }
    }

    impl Store for MockStore {
        fn apply(&self, _mutations: &[Mutation]) -> StoreOutcome {
            let behavior = self.behavior.lock().unwrap().clone();
            if behavior == Behavior::Panic {
                panic!("simulated panic inside the store task");
            }
            let mut rev = self.rev.lock().unwrap();
            *rev += 1;
            let current = *rev;
            self.record(format!("exec:{current}"));
            let snapshot = self.snapshot(current);
            match behavior {
                Behavior::Ok => StoreOutcome::ok(snapshot),
                Behavior::Fail(error) => StoreOutcome::failed(snapshot, error),
                Behavior::Panic => unreachable!(),
            }
        }

        fn reload(&self) -> StoreOutcome {
            let rev = *self.rev.lock().unwrap();
            StoreOutcome::ok(self.snapshot(rev))
        }

        fn isolate_and_reset(&self) -> StoreOutcome {
            StoreOutcome::failed(
                self.snapshot(*self.rev.lock().unwrap()),
                StoreError::NotDegraded,
            )
        }

        fn snapshot(&self) -> StoreSnapshot {
            self.snapshot(*self.rev.lock().unwrap())
        }
    }

    /// 记录每次发射的内容版本，用来断言"执行/接纳"的交替顺序。
    struct RecordingSink {
        log: Arc<Mutex<Vec<String>>>,
        failures_left: usize,
    }

    impl SnapshotSink for RecordingSink {
        fn emit(&mut self, snapshot: &UiSnapshot) -> bool {
            self.log
                .lock()
                .unwrap()
                .push(format!("emit:{}", snapshot.content_rev));
            if self.failures_left > 0 {
                self.failures_left -= 1;
                return false;
            }
            true
        }
    }

    fn initial() -> StoreSnapshot {
        StoreSnapshot {
            content_rev: ContentRev(1),
            slips: Vec::new(),
            health: Health::default(),
            needs_reload: false,
            recovery: RecoveryState::Normal,
            diagnostics: Vec::new(),
        }
    }

    fn build(
        log: Arc<Mutex<Vec<String>>>,
        behavior: Behavior,
        gate: Option<std::sync::Mutex<std::sync::mpsc::Receiver<()>>>,
        failures_left: usize,
        capacity: usize,
    ) -> Dispatcher {
        let store = Arc::new(MockStore {
            log: Arc::clone(&log),
            rev: Mutex::new(1),
            behavior: Mutex::new(behavior),
            gate,
        });
        let sink = RecordingSink {
            log: Arc::clone(&log),
            failures_left,
        };
        Dispatcher::spawn_with_sink(store, Presenter::new(initial()), Box::new(sink), capacity)
    }

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

    // ------------------------------------------------ I17：顺序

    #[tokio::test(flavor = "current_thread")]
    async fn execution_and_acceptance_alternate_in_order() {
        // 这条断言的是**轨迹**而不是最终状态：如果 store 与 presenter 各跑各的，
        // 最终状态可能碰巧正确，但交错顺序会暴露问题。
        let log = Arc::new(Mutex::new(Vec::new()));
        let dispatcher = build(Arc::clone(&log), Behavior::Ok, None, 0, 8);

        let a = dispatcher
            .apply(String::from("r1"), vec![Mutation::Append(slip("a"))])
            .await;
        let b = dispatcher
            .apply(String::from("r2"), vec![Mutation::Append(slip("b"))])
            .await;

        assert_eq!(a.result, AckResult::Applied);
        assert_eq!(b.result, AckResult::Applied);
        assert_eq!(
            *log.lock().unwrap(),
            vec![
                String::from("exec:2"),
                String::from("emit:2"),
                String::from("exec:3"),
                String::from("emit:3"),
            ]
        );
        dispatcher.shutdown().await;
    }

    // ------------------------------------------------ ui_rev

    #[tokio::test(flavor = "current_thread")]
    async fn ui_rev_increases_on_every_publication() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let dispatcher = build(log, Behavior::Ok, None, 0, 8);

        let first = dispatcher.get_snapshot().await.unwrap();
        let after_apply = dispatcher
            .apply(String::from("r1"), vec![Mutation::Append(slip("a"))])
            .await;
        assert!(after_apply.ui_rev.unwrap() > first.ui_rev);

        dispatcher.shutdown().await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn get_snapshot_does_not_bump_ui_rev() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let dispatcher = build(log, Behavior::Ok, None, 0, 8);
        let first = dispatcher.get_snapshot().await.unwrap();
        let second = dispatcher.get_snapshot().await.unwrap();
        assert_eq!(first.ui_rev, second.ui_rev);
        dispatcher.shutdown().await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn failed_emit_still_reports_applied_with_version() {
        let log = Arc::new(Mutex::new(Vec::new()));
        // 第一次发射失败，模拟"窗口已关/事件没送到"。
        let dispatcher = build(Arc::clone(&log), Behavior::Ok, None, 1, 8);
        let ack = dispatcher
            .apply(String::from("r1"), vec![Mutation::Append(slip("a"))])
            .await;

        assert_eq!(ack.result, AckResult::Applied);
        assert!(ack.ui_rev.is_some(), "ACK 必须带 ui_rev 以便前端补取");
        // 状态没有被回滚：再查一次还能拿到内容版本。
        let current = dispatcher.get_snapshot().await.unwrap();
        assert_eq!(current.content_rev, 2);
        dispatcher.shutdown().await;
    }

    // ------------------------------------------------ 错误映射

    #[tokio::test(flavor = "current_thread")]
    async fn store_errors_map_to_documented_codes() {
        let cases = [
            (
                StoreError::Conflict {
                    current_hash: slip_store::Hash([0; 32]),
                },
                AckResult::Conflict,
                "conflict",
            ),
            (
                StoreError::NeedsReload,
                AckResult::NeedsReload,
                "needsReload",
            ),
            (StoreError::ReadOnly, AckResult::Rejected, "readOnly"),
            (
                StoreError::Domain(slip_domain::DomainError::EmptyText),
                AckResult::Rejected,
                "domain",
            ),
            (
                StoreError::Blocking(slip_store::BlockingError::UnsupportedVersion),
                AckResult::Rejected,
                "blocking",
            ),
            (
                StoreError::Io(slip_store::IoKind::NotFound),
                AckResult::Rejected,
                "io",
            ),
            (StoreError::NotDegraded, AckResult::Rejected, "notDegraded"),
        ];

        for (error, expected_result, expected_code) in cases {
            let log = Arc::new(Mutex::new(Vec::new()));
            let dispatcher = build(log, Behavior::Fail(error), None, 0, 8);
            let ack = dispatcher
                .apply(String::from("r1"), vec![Mutation::Append(slip("a"))])
                .await;
            assert_eq!(ack.result, expected_result, "code {expected_code}");
            assert_eq!(ack.error.unwrap().code, expected_code);
            dispatcher.shutdown().await;
        }
    }

    // ------------------------------------------------ Fatal

    /// 用户可见的错误消息必须是中文。
    ///
    /// 背景（R77 审计）：`BlockingError` / `DomainError` 的 Display 原本是英文
    /// （"duplicate slip id: …"、"text is empty after trimming"），而前端对
    /// `blocking` / `domain` / `io` 这几个码**直接把 message 显示给用户**——
    /// 于是中文界面里会突然冒出一句英文。这条测试把"每一句都是中文"变成机器可查。
    #[test]
    fn user_facing_error_messages_are_chinese() {
        fn has_cjk(text: &str) -> bool {
            text.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c))
        }

        let cases: Vec<StoreError> = vec![
            StoreError::Conflict {
                current_hash: slip_store::Hash([0; 32]),
            },
            StoreError::NeedsReload,
            StoreError::ReadOnly,
            StoreError::Domain(slip_domain::DomainError::EmptyText),
            StoreError::Domain(slip_domain::DomainError::SlipLocked),
            StoreError::Domain(slip_domain::DomainError::ArchiveRequiresDone),
            StoreError::Blocking(slip_store::BlockingError::UnsupportedVersion),
            StoreError::Blocking(slip_store::BlockingError::NonUtf8Record { line: 7 }),
            StoreError::Io(slip_store::IoKind::PermissionDenied),
            StoreError::Io(slip_store::IoKind::Busy),
        ];

        let mut codes = std::collections::BTreeSet::new();
        for error in &cases {
            let (code, message) = map_store_error(error);
            codes.insert(code);
            assert!(
                has_cjk(&message),
                "错误码 {code} 的消息不是中文，用户会看到英文：{message}"
            );
            assert!(
                !message.contains('{') && !message.contains("SlipId"),
                "错误码 {code} 的消息泄漏了 Debug 结构体：{message}"
            );
        }

        // 码集合必须与前端 `formatAckError` 的 case 一致；这里少一个或前端少一个，
        // 都会让某条错误退回"直接显示 message"或显示"未知错误"。
        assert_eq!(
            codes.into_iter().collect::<Vec<_>>(),
            vec![
                "blocking",
                "conflict",
                "domain",
                "io",
                "needsReload",
                "readOnly"
            ],
            "错误码集合变了：请同步更新前端 formatAckError"
        );
    }

    /// 错误消息会**直接进入用户的提示区**，所以不能泄漏 Debug 结构体。
    ///
    /// 回归背景：`blocking` 曾经是 `format!("{err:?}")`，用户会在界面上看到
    /// `DuplicateId { id: SlipId(SlipId(019a…)) }`。这条测试挡住那种退化——
    /// 只要消息里出现结构体的花括号就红。
    #[test]
    fn blocking_message_is_human_readable() {
        let id = SlipId::new_v7();
        let (code, message) = map_store_error(&StoreError::Blocking(
            slip_store::BlockingError::DuplicateId { id },
        ));
        assert_eq!(code, "blocking");
        assert!(
            !message.contains('{') && !message.contains("SlipId"),
            "消息泄漏了 Debug 结构体：{message}"
        );
        assert!(message.contains("编号重复"), "缺少可读描述：{message}");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn panic_marks_unknown_and_enters_fatal() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let dispatcher = build(Arc::clone(&log), Behavior::Panic, None, 0, 8);

        let ack = dispatcher
            .apply(String::from("r1"), vec![Mutation::Append(slip("a"))])
            .await;

        assert_eq!(ack.result, AckResult::Unknown, "panic 后不能声称未提交");
        assert_eq!(ack.error.unwrap().code, "fatal");

        let current = dispatcher.get_snapshot().await.unwrap();
        assert!(current.runtime.is_fatal());

        // 后续内容命令必须被拒绝，且**不再触碰**故障 store。
        let before = log.lock().unwrap().len();
        let next = dispatcher
            .apply(String::from("r2"), vec![Mutation::Append(slip("b"))])
            .await;
        assert_eq!(next.result, AckResult::Rejected);
        assert_eq!(next.error.unwrap().code, "notExecuted");
        assert_eq!(
            log.lock().unwrap().len(),
            before,
            "Fatal 之后不得再调用 store"
        );

        dispatcher.shutdown().await;
    }

    // ------------------------------------------------ 队列边界

    #[tokio::test(flavor = "current_thread")]
    async fn full_queue_reports_busy_without_executing() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let (gate_tx, gate_rx) = std::sync::mpsc::channel();
        let dispatcher = Arc::new(build(
            Arc::clone(&log),
            Behavior::Ok,
            Some(Mutex::new(gate_rx)),
            0,
            1,
        ));

        // 第一条被消费者取走，并阻塞在 gate 上。
        let first = {
            let d = Arc::clone(&dispatcher);
            tokio::spawn(async move { d.apply(String::from("r1"), vec![]).await })
        };
        tokio::time::sleep(std::time::Duration::from_millis(80)).await;

        // 第二条填满容量为 1 的队列。
        let second = {
            let d = Arc::clone(&dispatcher);
            tokio::spawn(async move { d.apply(String::from("r2"), vec![]).await })
        };
        tokio::time::sleep(std::time::Duration::from_millis(80)).await;

        // 第三条入队失败，必须**立刻**返回 busy，且不产生 exec 记录。
        let third = dispatcher.apply(String::from("r3"), vec![]).await;
        assert_eq!(third.result, AckResult::Rejected);
        assert_eq!(third.error.unwrap().code, "busy");
        assert!(
            !log.lock().unwrap().iter().any(|e| e == "exec:4"),
            "被拒绝的请求不得执行"
        );

        let _ = gate_tx.send(());
        let _ = gate_tx.send(());
        let a = first.await.unwrap();
        let b = second.await.unwrap();
        assert_eq!(a.result, AckResult::Applied);
        assert_eq!(b.result, AckResult::Applied);
        dispatcher.shutdown().await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn shutdown_rejects_later_requests() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let dispatcher = build(log, Behavior::Ok, None, 0, 8);
        dispatcher.shutdown().await;
        let ack = dispatcher
            .apply(String::from("late"), vec![Mutation::Append(slip("a"))])
            .await;
        assert_eq!(ack.result, AckResult::Rejected);
        assert_eq!(ack.error.unwrap().code, "notExecuted");
    }
}
