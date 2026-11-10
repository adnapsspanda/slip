//! 内容与变更类型（v11 §1.2）。

use time::OffsetDateTime;

use crate::id::SlipId;

/// 内容版本号。
///
/// 进程内单调递增，**只在内容真正改变时递增**（成功修改、有效重载、恢复创建、
/// 接纳外部修复）。失败与 `Noop` 都不增。初始载入为 1，0 表示尚未载入。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ContentRev(pub u64);

/// 一张纸条的内容。**不含任何几何、展示或运行时状态。**
#[derive(Debug, Clone, PartialEq)]
pub struct Slip {
    pub id: SlipId,
    pub text: String,
    pub done: bool,
    pub created_at: OffsetDateTime,
    pub done_at: Option<OffsetDateTime>,
    pub archived_at: Option<OffsetDateTime>,
}

/// 拆条产出的草稿：正文 + 本次捕获的时间。
#[derive(Debug, Clone, PartialEq)]
pub struct Draft {
    pub text: String,
    pub occurred_at: OffsetDateTime,
}

/// 对已有纸条的局部修改。
#[derive(Debug, Clone, PartialEq)]
pub enum Patch {
    SetText(String),
    SetDone { done: bool, at: OffsetDateTime },
    Archive { at: OffsetDateTime },
}

/// 一次写入批次中的单个变更。
///
/// 一批 `Mutation` 由 store 作**全有或全无**处理，且最多触发一次 rename。
#[derive(Debug, Clone, PartialEq)]
pub enum Mutation {
    Append(Slip),
    Patch { id: SlipId, patch: Patch },
}

/// 转换是否真的改变了内容。
///
/// `Noop` 用于避免"重复点完成"产生多余写盘（v11 §1.7）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transition {
    Changed,
    Noop,
}
