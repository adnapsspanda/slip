//! Slip 的磁盘真源层。
//!
//! 这是**唯一**允许写 `inbox.md` 与 `ui-state.json` 的 crate（v11 I1）。
//! 不依赖 `tauri`：`cargo tree -p slip-store` 不得出现 `tauri`。

pub mod doc;
pub mod fsops;
pub mod prefs;

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

use slip_domain::{ContentRev, DomainError, Mutation, Slip, SlipId, validate_slip};
use time::OffsetDateTime;

/// 面向 UI 的一条诊断：文件无法载入时的位置与原因。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    /// 1 基行号；与具体文件无关的诊断为 `None`。
    pub line: Option<usize>,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Availability {
    #[default]
    ReadWrite,
    Degraded,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Warning {
    BackupFailed,
    UnknownField { line: usize },
    DurabilityUnconfirmed,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Health {
    pub availability: Availability,
    pub warnings: BTreeSet<Warning>,
}

impl Health {
    pub fn read_write() -> Self {
        Self::default()
    }

    pub fn degraded() -> Self {
        Self {
            availability: Availability::Degraded,
            warnings: BTreeSet::new(),
        }
    }

    pub fn is_writable(&self) -> bool {
        self.availability == Availability::ReadWrite
    }
}

/// 恢复状态机（v11 §3.4）。仅进程内维护，重启后按实际路径重新载入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoveryState {
    Normal,
    Isolated {
        corrupt_path: PathBuf,
        isolated_at: OffsetDateTime,
    },
}

/// 一次可发布的完整内容快照。
#[derive(Debug, Clone)]
pub struct StoreSnapshot {
    pub content_rev: ContentRev,
    pub slips: Vec<Slip>,
    pub health: Health,
    pub needs_reload: bool,
    pub recovery: RecoveryState,
    pub diagnostics: Vec<Diagnostic>,
}

/// 磁盘原始字节的 BLAKE3 摘要。含 BOM、CRLF、非记录行。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Hash(pub [u8; 32]);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockingError {
    InvalidRecord {
        line: usize,
        reason: doc::InvalidReason,
    },
    DuplicateId {
        id: SlipId,
    },
    UnsupportedVersion,
    NonUtf8Record {
        line: usize,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IoKind {
    NotFound,
    Occupied,
    PermissionDenied,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreError {
    Conflict { current_hash: Hash },
    NeedsReload,
    ReadOnly,
    Domain(DomainError),
    Blocking(BlockingError),
    Io(IoKind),
    NotDegraded,
}

/// 每次 `Store` 调用都返回它。**失败也必须携带可发布快照。**
#[derive(Debug, Clone)]
pub struct StoreOutcome {
    pub snapshot: StoreSnapshot,
    pub result: Result<ContentRev, StoreError>,
}

impl StoreOutcome {
    pub fn ok(snapshot: StoreSnapshot) -> Self {
        let rev = snapshot.content_rev;
        Self {
            snapshot,
            result: Ok(rev),
        }
    }

    pub fn failed(snapshot: StoreSnapshot, error: StoreError) -> Self {
        Self {
            snapshot,
            result: Err(error),
        }
    }

    pub fn is_applied(&self) -> bool {
        self.result.is_ok()
    }
}

/// 内容存储契约（v11 §3.1）。
///
/// 调用方必须串行调用（v11 §6.1 单队列）。实现内部仍需自持锁，
/// 因为 `snapshot()` 可能与写操作并发。
pub trait Store: Send + Sync {
    fn apply(&self, mutations: &[Mutation]) -> StoreOutcome;
    fn reload(&self) -> StoreOutcome;
    fn isolate_and_reset(&self) -> StoreOutcome;
    fn snapshot(&self) -> StoreSnapshot;
}

// ---------------------------------------------------------------------------
// 实现：磁盘事务层
// ---------------------------------------------------------------------------

/// 保留的备份代数（v11 §4.5）。
const BACKUP_KEEP: usize = 5;

/// `doc::parse` 在 `Store` 侧的样子：要么给出整份文档，要么给出阻断原因。
type ParseFn = fn(&[u8]) -> Result<doc::Parsed, BlockingError>;
/// 候选构造：在文档副本上应用一批 mutation，第二个返回值表示是否真的改变。
type ApplyFn = fn(&doc::Document, &[Mutation]) -> Result<(doc::Document, bool), doc::ApplyError>;
/// 由候选文档渲染候选字节。
type RenderFn = fn(&doc::Document) -> Vec<u8>;
/// 回读闭包：记录相等 + 未修改行字节相等。
type VerifyFn = fn(&doc::Document, &[u8], &[Slip]) -> Result<(), doc::RoundtripError>;

/// `doc.rs` 的四个纯函数。
///
/// 做成函数指针而不是直接调用，是为了让事务层能在 `doc.rs` 尚未完成时
/// 用构件化文档驱动测试；生产路径始终装配 [`DocOps::real`]，不存在行为旁路。
#[derive(Clone, Copy)]
struct DocOps {
    parse: ParseFn,
    apply: ApplyFn,
    render: RenderFn,
    verify: VerifyFn,
}

impl DocOps {
    fn real() -> Self {
        Self {
            parse: real_parse,
            apply: doc::apply,
            render: doc::render,
            verify: doc::verify_roundtrip,
        }
    }
}

/// 一次成功解析的产物：候选文档 + 磁盘字节 hash + 该文档的警告集合。
struct Accepted {
    document: doc::Document,
    hash: Hash,
    warnings: BTreeSet<Warning>,
}

/// 进程内的已载入状态。所有字段都在同一把锁下修改（v11 §3.2）。
struct Loaded {
    path: PathBuf,
    document: doc::Document,
    content_rev: ContentRev,
    health: Health,
    needs_reload: bool,
    recovery: RecoveryState,
    diagnostics: Vec<Diagnostic>,
    loaded_hash: Hash,
}

impl Loaded {
    fn snapshot(&self) -> StoreSnapshot {
        StoreSnapshot {
            content_rev: self.content_rev,
            slips: self
                .document
                .records
                .iter()
                .map(|record| record.slip.clone())
                .collect(),
            health: self.health.clone(),
            needs_reload: self.needs_reload,
            recovery: self.recovery.clone(),
            diagnostics: self.diagnostics.clone(),
        }
    }

    fn bump(&mut self) {
        self.content_rev = ContentRev(self.content_rev.0 + 1);
    }

    fn degrade_with(&mut self, diagnostic: Diagnostic) {
        self.health.availability = Availability::Degraded;
        self.diagnostics = vec![diagnostic];
    }
}

/// Markdown 真源的 [`Store`] 实现。
///
/// 状态机、检查顺序与提交点严格按 v11 §3、§4.4–§4.8：
///
/// - **提交点 = 同目录原子替换成功**（[`fsops::write_atomic`] 返回 `Ok`）；
/// - 替换成功之后发生的持久化确认失败只加 [`Warning::DurabilityUnconfirmed`]，
///   **绝不把已提交的写入报成 `Err`**（I14）；
/// - `apply` 的检查顺序是 `needs_reload` → `Degraded` → 候选校验 → 磁盘 hash → 备份提交；
/// - 永不 rename 已成功解析的文件，只对读得到的无效文件隔离（I20）。
pub struct MarkdownStore {
    state: Mutex<Loaded>,
    ops: DocOps,
}

impl MarkdownStore {
    /// 启动时载入：路径不存在则用 `create_new` 初始化空文件；`AlreadyExists` 则重读。
    ///
    /// 启动失败（读盘错误、解析失败）返回空内容 + `Degraded` + 诊断，
    /// **不返回部分解析结果**（v11 §3.4）。
    pub fn new(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let mut state = startup_state(path);
        // §4.5：启动时清理一次备份。
        if fsops::prune_backups(fsops::parent_dir(&state.path), BACKUP_KEEP).is_err() {
            state.health.warnings.insert(Warning::BackupFailed);
        }
        Self {
            state: Mutex::new(state),
            ops: DocOps::real(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Loaded> {
        // v11 §6.2：禁止在不可信状态下伪造 `StoreOutcome` 后继续写。
        // 锁中毒说明上一次写入中途 panic，调用方必须按 Fatal 处理；
        // 这里不 `clear_poison`，让 panic 继续向上传递。
        self.state.lock().expect("store mutex poisoned")
    }

    /// 解析整份文件字节并做 `Store` 侧的身份检查。
    fn parse_bytes(&self, bytes: &[u8]) -> Result<Accepted, BlockingError> {
        parse_with_identity(self.ops, bytes)
    }

    /// 只对**受影响的** `Slip` 跑 `validate_slip`（v11 §6.1 第 2 步）。
    ///
    /// 按**索引**判断"是否受影响"成立，前提是变更只有"追加"和"原位 Patch"：
    /// 记录既不重排也不删除，第 i 条永远对第 i 条，末尾新记录拿到 `None` 因而
    /// 必定被校验。若将来引入删除或重排，这里必须改成按 id 比对，否则会静默
    /// 漏校验——那种错误不会报错，只会让非法内容落盘。
    fn validate_candidate(
        &self,
        original: &doc::Document,
        candidate: &doc::Document,
    ) -> Result<(), StoreError> {
        for (index, record) in candidate.records.iter().enumerate() {
            let unchanged = original.records.get(index).map(|old| &old.slip) == Some(&record.slip);
            if unchanged {
                continue;
            }
            validate_slip(&record.slip).map_err(StoreError::Domain)?;
        }
        Ok(())
    }

    /// A 分支与 "转 A" 共用的重建：`create_new` → `AlreadyExists` 读一次 → 无效再隔离。
    fn rebuild_isolated(&self, state: &mut Loaded) -> StoreOutcome {
        match fsops::create_empty_new(&state.path) {
            Ok(()) => finish_fresh_create(state),
            Err(IoKind::Occupied) => {
                // A-2：只读一次现有文件。
                let bytes = match fsops::read_bytes(&state.path) {
                    Ok(bytes) => bytes,
                    Err(kind) => {
                        state.degrade_with(Diagnostic {
                            line: None,
                            message: format!("重建时读取 {} 失败：{kind:?}", state.path.display()),
                        });
                        return StoreOutcome::failed(state.snapshot(), StoreError::Io(kind));
                    }
                };
                match self.parse_bytes(&bytes) {
                    // 有效 → 接纳该文件，不 rename。
                    Ok(accepted) => {
                        accept(state, accepted);
                        StoreOutcome::ok(state.snapshot())
                    }
                    // 无效 → v11 §3.4 的死锁出口：再次隔离后重试创建。
                    Err(blocking) => {
                        let isolated_at = OffsetDateTime::now_utc();
                        match fsops::isolate(&state.path) {
                            Ok(corrupt_path) => {
                                state.health.availability = Availability::Degraded;
                                state.diagnostics = vec![describe_blocking(&blocking)];
                                state.recovery = RecoveryState::Isolated {
                                    corrupt_path,
                                    isolated_at,
                                };
                                match fsops::create_empty_new(&state.path) {
                                    Ok(()) => finish_fresh_create(state),
                                    Err(kind) => {
                                        // 保持 Isolated，保留两份证据。
                                        state.degrade_with(Diagnostic {
                                            line: None,
                                            message: format!(
                                                "重建 {} 失败：{kind:?}",
                                                state.path.display()
                                            ),
                                        });
                                        StoreOutcome::failed(state.snapshot(), StoreError::Io(kind))
                                    }
                                }
                            }
                            Err(kind) => {
                                StoreOutcome::failed(state.snapshot(), StoreError::Io(kind))
                            }
                        }
                    }
                }
            }
            Err(kind) => {
                // A-3：保留 `corrupt_path` 与 `Isolated`。
                state.degrade_with(Diagnostic {
                    line: None,
                    message: format!("创建 {} 失败：{kind:?}", state.path.display()),
                });
                StoreOutcome::failed(state.snapshot(), StoreError::Io(kind))
            }
        }
    }
}

impl Store for MarkdownStore {
    fn apply(&self, mutations: &[Mutation]) -> StoreOutcome {
        let mut state = self.lock();

        // 1. `needs_reload`：不读盘、不写盘。
        if state.needs_reload {
            return StoreOutcome::failed(state.snapshot(), StoreError::NeedsReload);
        }
        // 2. `Degraded`：不应用 mutation。
        if !state.health.is_writable() {
            return StoreOutcome::failed(state.snapshot(), StoreError::ReadOnly);
        }

        // 3. 在候选副本上应用并校验；全批 Noop 直接返回当前状态，不读盘。
        let (candidate, changed) = match (self.ops.apply)(&state.document, mutations) {
            Ok(applied) => applied,
            Err(error) => return StoreOutcome::failed(state.snapshot(), map_apply_error(error)),
        };
        if !changed {
            return StoreOutcome::ok(state.snapshot());
        }
        if let Err(error) = self.validate_candidate(&state.document, &candidate) {
            return StoreOutcome::failed(state.snapshot(), error);
        }

        // 候选字节必须"由原始行局部替换构造"并回读闭合（I13）。
        let bytes = (self.ops.render)(&candidate);
        let expected: Vec<Slip> = candidate
            .records
            .iter()
            .map(|record| record.slip.clone())
            .collect();
        if let Err(error) = (self.ops.verify)(&state.document, &bytes, &expected) {
            tracing::error!(?error, "候选回读闭包失败，拒绝写入");
            state.diagnostics = vec![Diagnostic {
                line: None,
                message: format!("候选回读闭包失败：{error:?}"),
            }];
            return StoreOutcome::failed(state.snapshot(), StoreError::Io(IoKind::Other));
        }

        // 4. 有变化才读盘比较原始 hash；不同则只置 `needs_reload`，不写任何字节。
        let current_hash = match fsops::read_bytes(&state.path) {
            Ok(disk) => fsops::hash_bytes(&disk),
            Err(kind) => {
                let message = format!("写入前读取 {} 失败：{kind:?}", state.path.display());
                state.degrade_with(Diagnostic {
                    line: None,
                    message,
                });
                return StoreOutcome::failed(state.snapshot(), StoreError::Io(kind));
            }
        };
        if current_hash != state.loaded_hash {
            state.needs_reload = true;
            return StoreOutcome::failed(state.snapshot(), StoreError::Conflict { current_hash });
        }

        // 5. 备份（失败只记警告，不阻断）→ 原子替换（提交点）。
        let backup_ok = fsops::backup(&state.path).is_ok();
        let prune_ok =
            backup_ok && fsops::prune_backups(fsops::parent_dir(&state.path), BACKUP_KEEP).is_ok();
        if backup_ok && prune_ok {
            state.health.warnings.remove(&Warning::BackupFailed);
        } else {
            state.health.warnings.insert(Warning::BackupFailed);
        }

        if let Err(kind) = fsops::write_atomic(&state.path, &bytes) {
            // 未提交：内存与 rev 都不变。
            return StoreOutcome::failed(state.snapshot(), StoreError::Io(kind));
        }

        // 6. 提交点已过。之后任何失败都只加 `DurabilityUnconfirmed`，不得报 Err。
        state.document = candidate;
        state.loaded_hash = fsops::hash_bytes(&bytes);
        state.bump();
        state.needs_reload = false;
        state
            .health
            .warnings
            .retain(|warning| !matches!(warning, Warning::UnknownField { .. }));
        let unknown_fields = unknown_field_warnings(&state.document);
        state.health.warnings.extend(unknown_fields);
        if fsops::sync_dir(fsops::parent_dir(&state.path)).is_err() {
            state.health.warnings.insert(Warning::DurabilityUnconfirmed);
        } else {
            state
                .health
                .warnings
                .remove(&Warning::DurabilityUnconfirmed);
        }

        StoreOutcome::ok(state.snapshot())
    }

    fn reload(&self) -> StoreOutcome {
        let mut state = self.lock();
        match fsops::read_bytes(&state.path) {
            Ok(bytes) => match self.parse_bytes(&bytes) {
                Ok(accepted) => {
                    accept(&mut state, accepted);
                    StoreOutcome::ok(state.snapshot())
                }
                Err(blocking) => {
                    // 失败：保留最后有效内容、hash 与隔离证据，只更新门禁与诊断；
                    // `needs_reload` 保留（§4.7）。
                    state.degrade_with(describe_blocking(&blocking));
                    StoreOutcome::failed(state.snapshot(), StoreError::Blocking(blocking))
                }
            },
            // 运行中 reload 遇到 NotFound 不自动建文件（§3.4）。
            Err(kind) => {
                let message = match kind {
                    IoKind::NotFound => {
                        format!("{} 已不存在，等待明确重建", state.path.display())
                    }
                    _ => format!("读取 {} 失败：{kind:?}", state.path.display()),
                };
                state.degrade_with(Diagnostic {
                    line: None,
                    message,
                });
                StoreOutcome::failed(state.snapshot(), StoreError::Io(kind))
            }
        }
    }

    fn isolate_and_reset(&self) -> StoreOutcome {
        let mut state = self.lock();
        match state.recovery.clone() {
            // A 分支：只重试重建，不回溯原路径。
            RecoveryState::Isolated { .. } => self.rebuild_isolated(&mut state),

            // B 分支：先检查当前可用性。
            RecoveryState::Normal => {
                if state.health.availability == Availability::ReadWrite {
                    return StoreOutcome::failed(state.snapshot(), StoreError::NotDegraded);
                }
                match fsops::read_bytes(&state.path) {
                    Ok(bytes) => match self.parse_bytes(&bytes) {
                        // B-3：外部已修复 → 接纳，不隔离。
                        Ok(accepted) => {
                            accept(&mut state, accepted);
                            StoreOutcome::ok(state.snapshot())
                        }
                        // B-4：解析失败 → 隔离到不覆盖的 corrupt-<ts>，置 Isolated，转 A。
                        Err(blocking) => {
                            let isolated_at = OffsetDateTime::now_utc();
                            match fsops::isolate(&state.path) {
                                Ok(corrupt_path) => {
                                    state.health.availability = Availability::Degraded;
                                    state.diagnostics = vec![describe_blocking(&blocking)];
                                    state.recovery = RecoveryState::Isolated {
                                        corrupt_path,
                                        isolated_at,
                                    };
                                    self.rebuild_isolated(&mut state)
                                }
                                Err(kind) => {
                                    StoreOutcome::failed(state.snapshot(), StoreError::Io(kind))
                                }
                            }
                        }
                    },
                    // B-5：无可隔离源文件 → 直接 create_new（含 `AlreadyExists` 按 A-2）。
                    Err(IoKind::NotFound) => self.rebuild_isolated(&mut state),
                    Err(kind) => StoreOutcome::failed(state.snapshot(), StoreError::Io(kind)),
                }
            }
        }
    }

    fn snapshot(&self) -> StoreSnapshot {
        self.lock().snapshot()
    }
}

// ---------------------------------------------------------------------------
// 内部状态转移
// ---------------------------------------------------------------------------

fn empty_document() -> doc::Document {
    doc::Document {
        bom: false,
        lines: Vec::new(),
        records: Vec::new(),
        preferred_terminator: None,
        ends_with_terminator: false,
    }
}

/// 启动载入。任何失败都落成"空内容 + `Degraded` + 诊断"，不返回部分结果。
fn startup_state(path: PathBuf) -> Loaded {
    startup_state_with(path, DocOps::real())
}

fn startup_state_with(path: PathBuf, ops: DocOps) -> Loaded {
    let mut state = Loaded {
        path,
        document: empty_document(),
        // 初始载入 = 1；0 表示尚未载入（§3.1）。
        content_rev: ContentRev(1),
        health: Health::read_write(),
        needs_reload: false,
        recovery: RecoveryState::Normal,
        diagnostics: Vec::new(),
        loaded_hash: fsops::hash_bytes(b""),
    };

    match fsops::read_bytes(&state.path) {
        Ok(bytes) => {
            adopt_at_startup(&mut state, &bytes, ops);
        }
        Err(IoKind::NotFound) => match fsops::create_empty_new(&state.path) {
            Ok(()) => {
                // 创建已是提交点；目录同步失败只表示持久性未确认。
                if fsops::sync_dir(fsops::parent_dir(&state.path)).is_err() {
                    state.health.warnings.insert(Warning::DurabilityUnconfirmed);
                }
            }
            Err(IoKind::Occupied) => match fsops::read_bytes(&state.path) {
                Ok(bytes) => adopt_at_startup(&mut state, &bytes, ops),
                Err(kind) => state.degrade_with(Diagnostic {
                    line: None,
                    message: format!("初始化 {} 后重读失败：{kind:?}", state.path.display()),
                }),
            },
            Err(kind) => state.degrade_with(Diagnostic {
                line: None,
                message: format!("初始化 {} 失败：{kind:?}", state.path.display()),
            }),
        },
        Err(kind) => state.degrade_with(Diagnostic {
            line: None,
            message: format!("启动读取 {} 失败：{kind:?}", state.path.display()),
        }),
    }

    state
}

fn adopt_at_startup(state: &mut Loaded, bytes: &[u8], ops: DocOps) {
    match parse_with_identity(ops, bytes) {
        Ok(accepted) => {
            state.document = accepted.document;
            state.loaded_hash = accepted.hash;
            state.health.warnings.extend(accepted.warnings);
        }
        Err(blocking) => {
            state.loaded_hash = fsops::hash_bytes(bytes);
            state.degrade_with(describe_blocking(&blocking));
        }
    }
}

/// 解析 + 重复 ID 检查。启动与运行中 reload 必须走同一条路径，
/// 否则坏文件在启动时会暂时被当成 `ReadWrite`。
fn parse_with_identity(ops: DocOps, bytes: &[u8]) -> Result<Accepted, BlockingError> {
    let parsed = (ops.parse)(bytes)?;
    let mut seen = BTreeSet::new();
    for record in &parsed.document.records {
        if !seen.insert(record.slip.id) {
            return Err(BlockingError::DuplicateId { id: record.slip.id });
        }
    }
    Ok(Accepted {
        warnings: unknown_field_warnings(&parsed.document),
        hash: fsops::hash_bytes(bytes),
        document: parsed.document,
    })
}

/// 接纳一份成功的解析结果（§3.4"接纳文件"）。
fn accept(state: &mut Loaded, accepted: Accepted) {
    state.document = accepted.document;
    state.loaded_hash = accepted.hash;
    state.health.availability = Availability::ReadWrite;
    state
        .health
        .warnings
        .retain(|warning| !matches!(warning, Warning::UnknownField { .. }));
    state.health.warnings.extend(accepted.warnings);
    state.needs_reload = false;
    state.recovery = RecoveryState::Normal;
    state.diagnostics.clear();
    state.bump();
}

/// `create_new` 成功后的提交：安装空文档。提交点是创建成功本身（§3.4）。
fn finish_fresh_create(state: &mut Loaded) -> StoreOutcome {
    state.document = empty_document();
    state.loaded_hash = fsops::hash_bytes(b"");
    // 接纳新空文档：清旧警告、旧解析诊断与恢复标记。
    state.health = Health::read_write();
    state.needs_reload = false;
    state.recovery = RecoveryState::Normal;
    state.diagnostics.clear();
    state.bump();

    // 创建已提交；之后的同步失败只附 `DurabilityUnconfirmed`。
    if fsops::sync_dir(fsops::parent_dir(&state.path)).is_err() {
        state.health.warnings.insert(Warning::DurabilityUnconfirmed);
    }

    StoreOutcome::ok(state.snapshot())
}

fn unknown_field_warnings(document: &doc::Document) -> BTreeSet<Warning> {
    let mut warnings = BTreeSet::new();
    for (index, line) in document.lines.iter().enumerate() {
        if let doc::LineKind::Record { index: record } = line.kind
            && document
                .records
                .get(record)
                .is_some_and(|record| !record.extras.is_empty())
        {
            warnings.insert(Warning::UnknownField { line: index + 1 });
        }
    }
    warnings
}

fn real_parse(bytes: &[u8]) -> Result<doc::Parsed, BlockingError> {
    let parsed = doc::parse(bytes);
    match parsed.problems.first() {
        None => Ok(parsed),
        Some(problem) => Err(match problem.reason {
            doc::InvalidReason::NonUtf8 => BlockingError::NonUtf8Record { line: problem.line },
            doc::InvalidReason::UnsupportedVersion => BlockingError::UnsupportedVersion,
            reason => BlockingError::InvalidRecord {
                line: problem.line,
                reason,
            },
        }),
    }
}

fn map_apply_error(error: doc::ApplyError) -> StoreError {
    match error {
        doc::ApplyError::Domain(error) => StoreError::Domain(error),
        doc::ApplyError::DuplicateId { id } => {
            StoreError::Blocking(BlockingError::DuplicateId { id })
        }
        // `SlipNotFound` 表示"调用方的记录集比文档旧"：拿一个**完全合法**的
        // UUIDv7 去 patch 一条已经不在文档里的纸条。
        //
        // 不能映射成 `Domain(DomainError::InvalidId)`——那个 ID 并没有问题，
        // `DomainError` 的契约是"只表达领域规则不满足"，而 `InvalidId` 的
        // Display 会输出 "invalid slip id"，对用户和日志都是假消息。
        //
        // 映射成 `NeedsReload` 才是对的：文件本身没变，变的是调用方视图，
        // 正确的下一步动作就是重新读取快照后重试。这里**刻意不置**
        // `needs_reload`：文档没有变化，不该因为这个理由冻结后续写入。
        doc::ApplyError::SlipNotFound { id } => {
            tracing::warn!(
                id = %id.to_canonical_string(),
                "mutation 指向当前文档中不存在的纸条，要求调用方重载后重试"
            );
            StoreError::NeedsReload
        }
    }
}

fn describe_blocking(error: &BlockingError) -> Diagnostic {
    match error {
        BlockingError::InvalidRecord { line, reason } => Diagnostic {
            line: Some(*line),
            message: format!("记录不合法：{}", reason_message(*reason)),
        },
        BlockingError::DuplicateId { id } => Diagnostic {
            line: None,
            message: format!("ID 重复：{}", id.to_canonical_string()),
        },
        BlockingError::UnsupportedVersion => Diagnostic {
            line: None,
            message: "记录版本不是 slip:v1".to_owned(),
        },
        BlockingError::NonUtf8Record { line } => Diagnostic {
            line: Some(*line),
            message: "记录候选行含非法 UTF-8".to_owned(),
        },
    }
}

fn reason_message(reason: doc::InvalidReason) -> &'static str {
    match reason {
        doc::InvalidReason::NonUtf8 => "含非法 UTF-8",
        doc::InvalidReason::BadCheckbox => "checkbox 不是 - [ ] / - [x] / - [X]",
        doc::InvalidReason::UnsupportedVersion => "头部不是 slip:v1",
        doc::InvalidReason::BadMarker => "marker 结构不完整",
        doc::InvalidReason::BadKeyValue => "kv 片段不满足 key=value",
        doc::InvalidReason::MissingField => "缺少必填键",
        doc::InvalidReason::DuplicateKey => "已知键重复",
        doc::InvalidReason::BadId => "id 不是合法 UUIDv7",
        doc::InvalidReason::BadTime => "时间无法按 RFC3339 往返",
        doc::InvalidReason::EmptyText => "文本去空白后为空",
        doc::InvalidReason::TextTooLong => "文本超长",
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;

    use slip_domain::{Patch, Transition, apply_patch};
    use time::macros::datetime;

    use super::*;

    // ---------- 构件化文档与 `doc` 函数 ----------------

    fn doc_literal() -> doc::Document {
        doc::Document {
            bom: false,
            lines: Vec::new(),
            records: Vec::new(),
            preferred_terminator: Some(doc::Terminator::Lf),
            ends_with_terminator: true,
        }
    }

    fn slip(text: &str) -> Slip {
        Slip {
            id: SlipId::new_v7(),
            text: text.to_owned(),
            done: false,
            created_at: datetime!(2026-11-10 12:00:00 +08:00),
            done_at: None,
            archived_at: None,
        }
    }

    /// 构件化 parse：空字节 → 空文档；含 `BAD!!` → 不合法；其余按行构造记录。
    fn fake_parse(bytes: &[u8]) -> Result<doc::Parsed, BlockingError> {
        if bytes.windows(5).any(|window| window == b"BAD!!") {
            return Err(BlockingError::InvalidRecord {
                line: 1,
                reason: doc::InvalidReason::BadCheckbox,
            });
        }
        let mut document = doc_literal();
        for chunk in bytes.split(|byte| *byte == b'\n') {
            if chunk.is_empty() {
                continue;
            }
            let index = document.records.len();
            let text = String::from_utf8_lossy(chunk).into_owned();
            document.records.push(doc::Record {
                slip: slip(&text),
                extras: Vec::new(),
            });
            document.lines.push(doc::Line {
                raw: chunk.to_vec(),
                terminator: doc::Terminator::Lf,
                kind: doc::LineKind::Record { index },
            });
        }
        Ok(doc::Parsed {
            document,
            problems: Vec::new(),
        })
    }

    /// 构件化 parse：把第一行标记为带未知字段。
    fn parse_with_unknown_field(bytes: &[u8]) -> Result<doc::Parsed, BlockingError> {
        let mut parsed = fake_parse(bytes)?;
        if let Some(record) = parsed.document.records.first_mut() {
            record.extras.push(doc::ExtraField {
                key: "extra".to_owned(),
                value: "a".to_owned(),
                raw: "extra=a".to_owned(),
            });
        }
        Ok(parsed)
    }

    fn fake_apply(
        document: &doc::Document,
        mutations: &[Mutation],
    ) -> Result<(doc::Document, bool), doc::ApplyError> {
        let mut next = document.clone();
        let mut changed = false;
        for mutation in mutations {
            match mutation {
                Mutation::Append(slip) => {
                    let index = next.records.len();
                    next.records.push(doc::Record {
                        slip: slip.clone(),
                        extras: Vec::new(),
                    });
                    next.lines.push(doc::Line {
                        raw: slip.text.clone().into_bytes(),
                        terminator: doc::Terminator::Lf,
                        kind: doc::LineKind::Record { index },
                    });
                    changed = true;
                }
                Mutation::Patch { id, patch } => {
                    let Some(position) = next.records.iter().position(|r| r.slip.id == *id) else {
                        return Err(doc::ApplyError::SlipNotFound { id: *id });
                    };
                    let (updated, transition) = apply_patch(&next.records[position].slip, patch)
                        .map_err(doc::ApplyError::Domain)?;
                    if transition == Transition::Changed {
                        next.records[position].slip = updated;
                        changed = true;
                    }
                }
            }
        }
        Ok((next, changed))
    }

    fn fake_render(document: &doc::Document) -> Vec<u8> {
        let mut out = Vec::new();
        for line in &document.lines {
            out.extend_from_slice(&line.raw);
            out.push(b'\n');
        }
        out
    }

    fn fake_verify(
        _original: &doc::Document,
        bytes: &[u8],
        expected: &[Slip],
    ) -> Result<(), doc::RoundtripError> {
        let parsed = fake_parse(bytes).map_err(|_| doc::RoundtripError::ReparseFailed)?;
        if parsed.document.records.len() != expected.len() {
            return Err(doc::RoundtripError::RecordMismatch { index: 0 });
        }
        for (index, (record, want)) in parsed
            .document
            .records
            .iter()
            .zip(expected.iter())
            .enumerate()
        {
            if record.slip.text != want.text {
                return Err(doc::RoundtripError::RecordMismatch { index });
            }
        }
        Ok(())
    }

    fn failing_verify(
        _original: &doc::Document,
        _bytes: &[u8],
        _expected: &[Slip],
    ) -> Result<(), doc::RoundtripError> {
        Err(doc::RoundtripError::ReparseFailed)
    }

    fn fake_ops() -> DocOps {
        DocOps {
            parse: fake_parse,
            apply: fake_apply,
            render: fake_render,
            verify: fake_verify,
        }
    }

    // ---------- 夹具 ----------

    struct Fixture {
        dir: tempfile::TempDir,
        path: PathBuf,
        store: MarkdownStore,
    }

    fn fixture_with(bytes: &[u8], document: doc::Document, ops: DocOps) -> Fixture {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("inbox.md");
        std::fs::write(&path, bytes).expect("write fixture");
        let store = MarkdownStore {
            state: Mutex::new(Loaded {
                path: path.clone(),
                document,
                content_rev: ContentRev(1),
                health: Health::read_write(),
                needs_reload: false,
                recovery: RecoveryState::Normal,
                diagnostics: Vec::new(),
                loaded_hash: fsops::hash_bytes(bytes),
            }),
            ops,
        };
        Fixture { dir, path, store }
    }

    fn fixture(bytes: &[u8]) -> Fixture {
        fixture_with(bytes, doc_literal(), fake_ops())
    }

    fn read(path: &Path) -> Vec<u8> {
        std::fs::read(path).expect("read back")
    }

    fn backups(dir: &Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .expect("list dir")
            .map(|entry| {
                entry
                    .expect("entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .filter(|name| name.contains(".bak-"))
            .collect()
    }

    fn corrupt_files(dir: &Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .expect("list dir")
            .map(|entry| {
                entry
                    .expect("entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .filter(|name| name.contains(".corrupt-"))
            .collect()
    }

    fn degrade(store: &MarkdownStore) {
        store.lock().health = Health::degraded();
    }

    fn mark_isolated(store: &MarkdownStore, corrupt_path: PathBuf) {
        let mut state = store.lock();
        state.health = Health::degraded();
        state.recovery = RecoveryState::Isolated {
            corrupt_path,
            isolated_at: datetime!(2026-11-10 12:00:00 +08:00),
        };
    }

    // ---------- 提交点（§4.4、I14） ----------

    #[test]
    fn commit_ack_survives_backup_failure() {
        let fixture = fixture(b"");
        fsops::failpoints::fail_next_backup();

        let outcome = fixture.store.apply(&[Mutation::Append(slip("买发票"))]);

        assert!(outcome.result.is_ok(), "备份失败不得把已提交写成失败");
        // 文件确实更新了：写入的是候选文档渲染出的字节。
        assert_eq!(read(&fixture.path), "买发票\n".as_bytes());
        assert!(
            outcome
                .snapshot
                .health
                .warnings
                .contains(&Warning::BackupFailed)
        );
        assert_eq!(outcome.snapshot.content_rev, ContentRev(2));
    }

    #[test]
    fn commit_ack_survives_directory_sync_failure() {
        let fixture = fixture(b"");
        fsops::failpoints::fail_next_dir_sync();

        let outcome = fixture.store.apply(&[Mutation::Append(slip("买发票"))]);

        assert!(outcome.result.is_ok(), "目录同步失败不得把已提交写成失败");
        assert!(!read(&fixture.path).is_empty());
        assert!(
            outcome
                .snapshot
                .health
                .warnings
                .contains(&Warning::DurabilityUnconfirmed)
        );
        assert_eq!(outcome.snapshot.content_rev, ContentRev(2));

        // 下一次同步成功的提交清除该警告。
        let again = fixture.store.apply(&[Mutation::Append(slip("第二张"))]);
        assert!(again.result.is_ok());
        assert!(
            !again
                .snapshot
                .health
                .warnings
                .contains(&Warning::DurabilityUnconfirmed)
        );
    }

    // ---------- 冲突与 needs_reload（§4.7、I8） ----------

    #[test]
    fn external_edit_produces_conflict_without_writing() {
        let fixture = fixture(b"");
        let first = fixture.store.apply(&[Mutation::Append(slip("第一张"))]);
        assert!(first.result.is_ok());

        std::fs::write(&fixture.path, b"external\n").expect("external edit");
        let before = read(&fixture.path);

        let outcome = fixture.store.apply(&[Mutation::Append(slip("第二张"))]);

        match outcome.result {
            Err(StoreError::Conflict { current_hash }) => {
                assert_eq!(current_hash, fsops::hash_bytes(&before));
            }
            other => panic!("期望 Conflict，实际 {other:?}"),
        }
        assert_eq!(read(&fixture.path), before, "冲突时不得写入任何字节");
        assert!(outcome.snapshot.needs_reload);
        assert_eq!(outcome.snapshot.content_rev, ContentRev(2));
    }

    #[test]
    fn needs_reload_blocks_apply_without_reading_disk() {
        let fixture = fixture(b"");
        fixture.store.lock().needs_reload = true;
        // 删掉文件：若实现真的读盘，这里会得到 Io(NotFound) 而不是 NeedsReload。
        std::fs::remove_file(&fixture.path).expect("remove");

        let outcome = fixture.store.apply(&[Mutation::Append(slip("不该写入"))]);

        assert_eq!(outcome.result, Err(StoreError::NeedsReload));
        assert_eq!(outcome.snapshot.content_rev, ContentRev(1));
        assert!(!fixture.path.exists());
    }

    #[test]
    fn degraded_store_rejects_apply_as_read_only() {
        let fixture = fixture(b"");
        degrade(&fixture.store);

        let outcome = fixture.store.apply(&[Mutation::Append(slip("不该写入"))]);

        assert_eq!(outcome.result, Err(StoreError::ReadOnly));
        assert_eq!(read(&fixture.path), b"");
        assert_eq!(outcome.snapshot.content_rev, ContentRev(1));
    }

    // ---------- Noop（§1.7） ----------

    #[test]
    fn noop_batch_does_not_touch_disk() {
        let fixture = fixture(b"");
        let outcome = fixture.store.apply(&[]);

        assert!(outcome.result.is_ok());
        assert_eq!(outcome.snapshot.content_rev, ContentRev(1));
        assert_eq!(read(&fixture.path), b"");
        assert!(backups(fixture.dir.path()).is_empty(), "Noop 不备份");
    }

    #[test]
    fn noop_patch_does_not_bump_rev() {
        let existing = slip("买发票");
        let mut document = doc_literal();
        document.records.push(doc::Record {
            slip: existing.clone(),
            extras: Vec::new(),
        });
        let bytes = fake_render(&document);
        let fixture = fixture_with(&bytes, document, fake_ops());

        let outcome = fixture.store.apply(&[Mutation::Patch {
            id: existing.id,
            patch: Patch::SetText(existing.text.clone()),
        }]);

        assert!(outcome.result.is_ok());
        assert_eq!(outcome.snapshot.content_rev, ContentRev(1));
        assert!(backups(fixture.dir.path()).is_empty());
    }

    // ---------- 候选闭包失败 ----------

    #[test]
    fn roundtrip_failure_writes_nothing() {
        let fixture = fixture_with(
            b"",
            doc_literal(),
            DocOps {
                verify: failing_verify,
                ..fake_ops()
            },
        );

        let outcome = fixture.store.apply(&[Mutation::Append(slip("买发票"))]);

        assert!(outcome.result.is_err());
        assert_eq!(read(&fixture.path), b"", "闭包失败不得写盘");
        assert_eq!(outcome.snapshot.content_rev, ContentRev(1));
    }

    // ---------- 并发（I12 内部串行） ----------

    #[test]
    fn concurrent_applies_serialize() {
        let fixture = fixture(b"");
        let store = Arc::new(fixture.store);
        let path = fixture.path.clone();

        let handles: Vec<_> = (0..8)
            .map(|index| {
                let store = Arc::clone(&store);
                std::thread::spawn(move || {
                    store.apply(&[Mutation::Append(slip(&format!("并发 {index}")))])
                })
            })
            .collect();

        for handle in handles {
            let outcome = handle.join().expect("join");
            assert!(outcome.result.is_ok(), "并发提交不得失败");
        }

        let snapshot = store.snapshot();
        assert_eq!(snapshot.slips.len(), 8);
        assert_eq!(snapshot.content_rev, ContentRev(9));

        let parsed = fake_parse(&read(&path)).expect("最终文件应可解析");
        assert_eq!(parsed.document.records.len(), 8);
    }

    // ---------- 恢复矩阵（§3.4、§9.1 第 3 条） ----------

    #[test]
    fn isolate_requires_degraded() {
        let fixture = fixture(b"keep\n");
        let outcome = fixture.store.isolate_and_reset();

        assert_eq!(outcome.result, Err(StoreError::NotDegraded));
        assert_eq!(read(&fixture.path), b"keep\n");
        assert!(corrupt_files(fixture.dir.path()).is_empty());
        assert_eq!(outcome.snapshot.content_rev, ContentRev(1));
    }

    #[test]
    fn isolated_with_missing_file_creates_empty() {
        let fixture = fixture(b"");
        std::fs::remove_file(&fixture.path).expect("remove");
        mark_isolated(
            &fixture.store,
            fixture.dir.path().join("inbox.md.corrupt-1"),
        );

        let outcome = fixture.store.isolate_and_reset();

        assert!(outcome.result.is_ok());
        assert_eq!(outcome.snapshot.recovery, RecoveryState::Normal);
        assert_eq!(
            outcome.snapshot.health.availability,
            Availability::ReadWrite
        );
        assert_eq!(outcome.snapshot.content_rev, ContentRev(2));
        assert_eq!(read(&fixture.path), b"");
    }

    #[test]
    fn isolated_with_existing_valid_file_accepts_it() {
        let fixture = fixture(b"external fix\n");
        mark_isolated(
            &fixture.store,
            fixture.dir.path().join("inbox.md.corrupt-1"),
        );

        let outcome = fixture.store.isolate_and_reset();

        assert!(outcome.result.is_ok());
        assert_eq!(outcome.snapshot.recovery, RecoveryState::Normal);
        assert_eq!(outcome.snapshot.content_rev, ContentRev(2));
        assert_eq!(outcome.snapshot.slips.len(), 1);
        assert_eq!(outcome.snapshot.slips[0].text, "external fix");
        assert_eq!(read(&fixture.path), b"external fix\n", "有效文件不得被移动");
        assert!(corrupt_files(fixture.dir.path()).is_empty());
    }

    #[test]
    fn isolated_with_invalid_file_reisolates_and_rebuilds() {
        let fixture = fixture(b"BAD!! broken\n");
        let corrupt = fixture.dir.path().join("inbox.md.corrupt-1");
        mark_isolated(&fixture.store, corrupt);

        let outcome = fixture.store.isolate_and_reset();

        assert!(outcome.result.is_ok());
        assert_eq!(outcome.snapshot.recovery, RecoveryState::Normal);
        assert_eq!(
            outcome.snapshot.health.availability,
            Availability::ReadWrite
        );
        assert_eq!(read(&fixture.path), b"", "重建后是空文件");
        assert_eq!(
            corrupt_files(fixture.dir.path()).len(),
            1,
            "无效文件必须被再次隔离"
        );
        let isolated = outcome.snapshot.recovery.clone();
        assert!(matches!(isolated, RecoveryState::Normal));
    }

    #[test]
    fn invalid_file_isolated_with_real_parser() {
        // 真实 parser 判定有效性。
        // 样本必须是**记录候选行**（`- [` 前缀 + ` <!-- slip:`）但文法不合法：
        // id 不是 UUIDv7，按 §4.2 应为 `InvalidRecord`，不得被当成说明行静默保留。
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("inbox.md");
        std::fs::write(
            &path,
            b"- [ ] broken <!-- slip:v1;id=not-a-uuid;created=2026-11-10T11:49:14+08:00 -->\n",
        )
        .expect("write");
        let store = MarkdownStore {
            state: Mutex::new(Loaded {
                path: path.clone(),
                document: doc_literal(),
                content_rev: ContentRev(1),
                health: Health::degraded(),
                needs_reload: false,
                recovery: RecoveryState::Isolated {
                    corrupt_path: dir.path().join("inbox.md.corrupt-1"),
                    isolated_at: OffsetDateTime::now_utc(),
                },
                diagnostics: Vec::new(),
                loaded_hash: fsops::hash_bytes(b""),
            }),
            ops: DocOps::real(),
        };

        let outcome = store.isolate_and_reset();

        assert!(outcome.result.is_ok());
        assert_eq!(outcome.snapshot.recovery, RecoveryState::Normal);
        assert!(fsops::read_bytes(&path).unwrap().is_empty());
        assert_eq!(corrupt_files(dir.path()).len(), 1);
    }

    #[test]
    fn degraded_external_fix_is_accepted() {
        let fixture = fixture_with(
            b"fixed\n",
            doc_literal(),
            DocOps {
                parse: parse_with_unknown_field,
                ..fake_ops()
            },
        );
        degrade(&fixture.store);

        let outcome = fixture.store.isolate_and_reset();

        assert!(outcome.result.is_ok());
        assert_eq!(outcome.snapshot.recovery, RecoveryState::Normal);
        assert_eq!(
            outcome.snapshot.health.availability,
            Availability::ReadWrite
        );
        assert_eq!(outcome.snapshot.content_rev, ContentRev(2));
        assert_eq!(read(&fixture.path), b"fixed\n", "外部修复不得被隔离");
        assert!(corrupt_files(fixture.dir.path()).is_empty());
        assert!(
            outcome
                .snapshot
                .health
                .warnings
                .contains(&Warning::UnknownField { line: 1 }),
            "接纳外部修复必须重算 UnknownField，不能一律清空"
        );
    }

    #[test]
    fn degraded_invalid_file_is_isolated_then_rebuilt() {
        let fixture = fixture(b"BAD!! broken\n");
        degrade(&fixture.store);

        let outcome = fixture.store.isolate_and_reset();

        assert!(outcome.result.is_ok());
        assert_eq!(outcome.snapshot.recovery, RecoveryState::Normal);
        assert_eq!(read(&fixture.path), b"");
        assert_eq!(
            corrupt_files(fixture.dir.path()).len(),
            1,
            "无效文件必须先隔离再重建"
        );
    }

    #[test]
    fn degraded_with_missing_file_creates_empty() {
        let fixture = fixture(b"");
        std::fs::remove_file(&fixture.path).expect("remove");
        degrade(&fixture.store);

        let outcome = fixture.store.isolate_and_reset();

        assert!(outcome.result.is_ok());
        assert_eq!(outcome.snapshot.recovery, RecoveryState::Normal);
        assert_eq!(read(&fixture.path), b"");
        assert_eq!(outcome.snapshot.content_rev, ContentRev(2));
    }

    // ---------- reload（§3.4、§4.7） ----------

    #[test]
    fn reload_accepts_and_clears_needs_reload() {
        let fixture = fixture(b"one\ntwo\n");
        {
            let mut state = fixture.store.lock();
            state.needs_reload = true;
            state.health = Health::degraded();
        }

        let outcome = fixture.store.reload();

        assert!(outcome.result.is_ok());
        assert!(!outcome.snapshot.needs_reload);
        assert_eq!(
            outcome.snapshot.health.availability,
            Availability::ReadWrite
        );
        assert_eq!(outcome.snapshot.slips.len(), 2);
        assert_eq!(outcome.snapshot.content_rev, ContentRev(2));
    }

    #[test]
    fn reload_failure_keeps_last_content_and_gate() {
        let fixture = fixture(b"original\n");
        std::fs::write(&fixture.path, b"BAD!! broken\n").expect("break");
        fixture.store.lock().needs_reload = true;

        let outcome = fixture.store.reload();

        assert!(matches!(outcome.result, Err(StoreError::Blocking(_))));
        assert_eq!(outcome.snapshot.content_rev, ContentRev(1));
        assert_eq!(outcome.snapshot.health.availability, Availability::Degraded);
        assert!(
            outcome.snapshot.needs_reload,
            "失败 reload 保留 needs_reload"
        );
        assert_eq!(outcome.snapshot.diagnostics[0].line, Some(1));
    }

    #[test]
    fn reload_missing_file_does_not_create() {
        let fixture = fixture(b"");
        std::fs::remove_file(&fixture.path).expect("remove");

        let outcome = fixture.store.reload();

        assert_eq!(outcome.result, Err(StoreError::Io(IoKind::NotFound)));
        assert_eq!(outcome.snapshot.health.availability, Availability::Degraded);
        assert!(!fixture.path.exists(), "reload 不得自动建文件");
    }

    // ---------- 真实 doc.rs 的端到端 ----------

    #[test]
    fn real_pipeline_appends_and_preserves_untouched_lines() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("inbox.md");
        let original = "# Slip\n\n- [ ] 手工写的说明行\n";
        std::fs::write(&path, original).expect("write");

        let store = MarkdownStore::new(&path);
        assert_eq!(store.snapshot().content_rev, ContentRev(1));
        assert!(store.snapshot().health.is_writable());

        let outcome = store.apply(&[Mutation::Append(slip("买发票"))]);
        assert!(outcome.result.is_ok(), "结果：{:?}", outcome.result);
        assert_eq!(outcome.snapshot.content_rev, ContentRev(2));
        assert_eq!(outcome.snapshot.slips.len(), 1);

        let written = String::from_utf8(read(&path)).expect("utf8");
        assert!(
            written.starts_with(original),
            "未修改行必须逐字节保留，实际：{written:?}"
        );
        assert!(written.contains("<!-- slip:v1;"));
        assert!(written.contains("买发票"));

        // 回读闭包与重新载入一致。
        let reloaded = store.reload();
        assert!(reloaded.result.is_ok(), "结果：{:?}", reloaded.result);
        assert_eq!(reloaded.snapshot.slips, outcome.snapshot.slips);
        assert_eq!(String::from_utf8(read(&path)).expect("utf8"), written);
        assert!(backups(dir.path()).len() == 1, "一次提交一次备份");
    }

    #[test]
    fn real_pipeline_startup_creates_missing_file() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("inbox.md");

        let store = MarkdownStore::new(&path);

        assert!(path.exists(), "启动时缺失应 create_new 初始化");
        assert_eq!(read(&path), b"");
        assert_eq!(store.snapshot().content_rev, ContentRev(1));
        assert!(store.snapshot().health.is_writable());
    }

    #[test]
    fn startup_create_sync_failure_warns_but_keeps_the_file() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("inbox.md");
        fsops::failpoints::fail_next_dir_sync();

        let store = MarkdownStore::new(&path);

        assert!(path.exists(), "创建成功即提交点，不能因同步失败回滚");
        assert!(
            store
                .snapshot()
                .health
                .warnings
                .contains(&Warning::DurabilityUnconfirmed)
        );
    }

    #[test]
    fn startup_with_duplicate_ids_is_degraded_and_empty() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("inbox.md");
        let line = "- [ ] t <!-- slip:v1;id=019a9f3c-2b70-7d84-a1c6-8e3f5b0d2741;created=2026-11-10T11:49:14+08:00 -->\n";
        std::fs::write(&path, format!("{line}{line}")).expect("write duplicate file");

        let store = MarkdownStore::new(&path);
        let snapshot = store.snapshot();

        assert!(!snapshot.health.is_writable());
        assert!(snapshot.slips.is_empty(), "启动失败不得返回部分解析结果");
        assert!(
            snapshot
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("ID 重复")),
            "diagnostics: {:?}",
            snapshot.diagnostics
        );
    }
}
