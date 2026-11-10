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
use std::time::Duration;

use slip_domain::{Mutation, Patch};
use slip_store::prefs::{Prefs, PrefsStore};
use slip_store::{Store, StoreError, StoreSnapshot};
use time::OffsetDateTime;
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::{mpsc, oneshot};

use crate::presenter::{Presenter, ViewEvent};
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
    ToggleCollapse {
        request_id: String,
        id: slip_domain::SlipId,
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
    GetSnapshot {
        reply: oneshot::Sender<Result<UiSnapshot, QueryError>>,
    },
    ViewEvent(ViewEvent),
    FlushPrefs,
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
    pub fn spawn(
        store: Arc<dyn Store>,
        prefs: Arc<dyn PrefsStore>,
        presenter: Presenter,
        capacity: usize,
    ) -> Self {
        Self::spawn_with_sink(store, prefs, presenter, Box::new(NoopSink), capacity)
    }

    fn spawn_with_sink(
        store: Arc<dyn Store>,
        prefs: Arc<dyn PrefsStore>,
        presenter: Presenter,
        sink: Box<dyn SnapshotSink>,
        capacity: usize,
    ) -> Self {
        let (tx, rx) = mpsc::channel(capacity);
        tokio::spawn(run(rx, store, prefs, presenter, sink));
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

    pub async fn toggle_collapse(&self, request_id: String, id: slip_domain::SlipId) -> Ack {
        self.submit(request_id, move |rid, reply| Command::ToggleCollapse {
            request_id: rid,
            id,
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

    /// 视图事件是 fire-and-forget：没有应答，也**不保证送达**。
    pub fn view_event(&self, event: ViewEvent) {
        let _ = self.tx.try_send(Command::ViewEvent(event));
    }

    pub fn flush_prefs(&self) {
        let _ = self.tx.try_send(Command::FlushPrefs);
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
    prefs: Arc<dyn PrefsStore>,
    mut presenter: Presenter,
    mut sink: Box<dyn SnapshotSink>,
) {
    const PREFS_DEBOUNCE: Duration = Duration::from_secs(1);
    /// 撤销栈深度。只在内存里，重启即清空。
    const UNDO_DEPTH: usize = 50;
    let mut undo: VecDeque<Vec<Mutation>> = VecDeque::new();
    let mut prefs_dirty = false;
    let mut prefs_due: Option<tokio::time::Instant> = None;

    loop {
        let received = if let Some(due) = prefs_due {
            tokio::select! {
                command = rx.recv() => command,
                _ = tokio::time::sleep_until(due) => {
                    if prefs_dirty && !presenter.is_fatal() {
                        prefs_dirty = !persist_prefs(&prefs, presenter.view().to_prefs()).await;
                    }
                    prefs_due = None;
                    continue;
                }
            }
        } else {
            rx.recv().await
        };
        let Some(command) = received else {
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

            Command::Reload { request_id, reply } => {
                if presenter.is_fatal() {
                    let _ = reply.send(not_executed(request_id));
                    continue;
                }
                let store_handle = Arc::clone(&store);
                let result = tokio::task::spawn_blocking(move || store_handle.reload()).await;
                let ack = match result {
                    Ok(outcome) => {
                        let error = outcome.result.as_ref().err().map(map_store_error);
                        presenter.accept(&outcome);
                        let ui_rev = presenter.ui_rev();
                        emit(&presenter, &mut *sink);
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

            Command::IsolateAndReset { request_id, reply } => {
                if presenter.is_fatal() {
                    let _ = reply.send(not_executed(request_id));
                    continue;
                }
                let store_handle = Arc::clone(&store);
                let result =
                    tokio::task::spawn_blocking(move || store_handle.isolate_and_reset()).await;
                let ack = match result {
                    Ok(outcome) => {
                        let error = outcome.result.as_ref().err().map(map_store_error);
                        presenter.accept(&outcome);
                        let ui_rev = presenter.ui_rev();
                        emit(&presenter, &mut *sink);
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

            // 折叠只改展示，不碰磁盘；但它**会**改变几何，因此必须发布新版本。
            Command::ToggleCollapse {
                request_id,
                id,
                reply,
            } => {
                if presenter.is_fatal() {
                    let _ = reply.send(not_executed(request_id));
                    continue;
                }
                presenter.toggle_collapse(id);
                prefs_dirty = true;
                prefs_due = Some(tokio::time::Instant::now() + PREFS_DEBOUNCE);
                let ui_rev = presenter.ui_rev();
                emit(&presenter, &mut *sink);
                let _ = reply.send(Ack {
                    request_id,
                    result: AckResult::Applied,
                    ui_rev: Some(ui_rev),
                    error: None,
                });
            }

            // 视图事件没有应答；失败也不影响内容。
            Command::ViewEvent(event) => {
                if !presenter.is_fatal() {
                    presenter.apply_view_event(&event);
                    emit(&presenter, &mut *sink);
                    if view_event_dirties_prefs(&event) {
                        prefs_dirty = true;
                        prefs_due = Some(tokio::time::Instant::now() + PREFS_DEBOUNCE);
                    }
                }
            }

            Command::FlushPrefs => {
                if prefs_dirty && !presenter.is_fatal() {
                    prefs_dirty = !persist_prefs(&prefs, presenter.view().to_prefs()).await;
                }
                prefs_due = None;
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
                if prefs_dirty && !presenter.is_fatal() {
                    let _ = persist_prefs(&prefs, presenter.view().to_prefs()).await;
                }
                let _ = reply.send(());
                break;
            }
        }
    }
}

/// 偏好保存失败只记日志并保持 dirty；下一次变更或退出时再试一次。
async fn persist_prefs(prefs: &Arc<dyn PrefsStore>, value: Prefs) -> bool {
    let handle = Arc::clone(prefs);
    match tokio::task::spawn_blocking(move || handle.save_prefs(&value)).await {
        Ok(Ok(())) => true,
        Ok(Err(error)) => {
            tracing::warn!(?error, "prefs save failed; keeping dirty");
            false
        }
        Err(error) => {
            tracing::error!(%error, "prefs save task failed; keeping dirty");
            false
        }
    }
}

fn view_event_dirties_prefs(event: &ViewEvent) -> bool {
    matches!(
        event,
        ViewEvent::DragEnd { .. }
            | ViewEvent::DockSideChanged(_)
            | ViewEvent::CollapseToggled { .. }
    )
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
        Ok(outcome) => {
            let error = outcome.result.as_ref().err().map(map_store_error);
            presenter.accept(&outcome);
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
        // 得到 JoinError 意味着我们无法知道磁盘上到底发生了什么：可能 rename
        // 已经成功。只能报 unknown 并进入 Fatal。
        Err(join) => fatal(presenter, sink, request_id, join.to_string()),
    }
}

/// 这批变更是否值得进撤销栈。
///
/// 覆盖"改正文""改完成状态"与"归档"：归档后纸条会从默认视图消失，没有原地
/// 按钮可点回去，必须允许 `Ctrl+Z` 恢复；追加没有删除变更，锁定有显式开关。
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
            Patch::SetLocked(_) => return None,
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
        | Command::ToggleCollapse {
            request_id, reply, ..
        }
        | Command::Reload { request_id, reply }
        | Command::Undo { request_id, reply }
        | Command::IsolateAndReset { request_id, reply } => {
            let _ = reply.send(not_executed(request_id));
        }
        Command::GetSnapshot { reply } => {
            let _ = reply.send(Err(QueryError(String::from("notExecuted"))));
        }
        // 没有应答句柄的命令无从通知，直接丢弃。
        Command::ViewEvent(_) | Command::FlushPrefs => {}
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
        StoreError::Conflict { .. } => (
            "conflict",
            String::from("file changed on disk since it was loaded"),
        ),
        StoreError::NeedsReload => (
            "needsReload",
            String::from("reload is required before writing again"),
        ),
        StoreError::ReadOnly => (
            "readOnly",
            String::from("the file cannot be parsed; vault is read-only"),
        ),
        StoreError::Domain(err) => ("domain", err.to_string()),
        // 用 Display 而不是 Debug：这条消息会直接出现在用户的提示区。
        StoreError::Blocking(err) => ("blocking", err.to_string()),
        StoreError::Io(kind) => ("io", format!("i/o error: {kind:?}")),
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
    use std::collections::BTreeSet;
    use std::sync::{Arc, Mutex};

    use slip_domain::{ContentRev, Slip, SlipId};
    use slip_store::prefs::Prefs;
    use slip_store::{Availability, Health, RecoveryState, StoreOutcome, StoreSnapshot};
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

    struct MockPrefs;

    impl PrefsStore for MockPrefs {
        fn load_prefs(&self) -> Prefs {
            Prefs::default()
        }

        fn save_prefs(&self, _prefs: &Prefs) -> Result<(), slip_store::prefs::PrefsError> {
            Ok(())
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
        gate: Option<Mutex<std::sync::mpsc::Receiver<()>>>,
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
        Dispatcher::spawn_with_sink(
            store,
            Arc::new(MockPrefs),
            Presenter::new(initial()),
            Box::new(sink),
            capacity,
        )
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

        // 视图事件没有 ACK，但**必须**递增版本，否则前端会按 ui_rev 去重丢弃它。
        dispatcher.view_event(ViewEvent::MonitorsChanged(
            crate::presenter::MonitorSnapshot::default(),
        ));
        tokio::task::yield_now().await;

        let latest = dispatcher.get_snapshot().await.unwrap();
        assert!(latest.ui_rev > after_apply.ui_rev.unwrap());
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
        assert!(message.contains("duplicate"), "缺少可读描述：{message}");
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

    #[tokio::test(flavor = "current_thread")]
    async fn toggle_collapse_publishes_without_touching_store() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let dispatcher = build(Arc::clone(&log), Behavior::Ok, None, 0, 8);

        let ack = dispatcher
            .toggle_collapse(String::from("r1"), SlipId::new_v7())
            .await;
        assert_eq!(ack.result, AckResult::Applied);
        assert!(
            log.lock().unwrap().iter().all(|e| !e.starts_with("exec:")),
            "折叠不应触发磁盘写入"
        );
        dispatcher.shutdown().await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn health_defaults_to_writable_and_warnings_empty() {
        // 防止有人把 Health::default() 改成 Degraded 而静默禁掉所有写入。
        let health = Health::default();
        assert_eq!(health.availability, Availability::ReadWrite);
        assert_eq!(health.warnings, BTreeSet::new());
    }
}
