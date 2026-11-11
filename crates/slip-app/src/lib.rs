//! Slip 的调度与视图层。v1 **不依赖 `tauri`**，以便纯逻辑可测。
//!
//! 对应 v11 §6：单命令队列、presenter 单一所有者、`ui_rev` 递增规则、
//! ACK 映射、四条件清空。

pub mod dispatch;
pub mod geometry;
pub mod presenter;
pub mod tray;

use std::collections::BTreeSet;

use serde::Serialize;

/// 运行时状态。属于 app/UI，**不属于 Markdown 或 `StoreSnapshot`**。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum RuntimeStatus {
    Running,
    Fatal { reason: String },
}

impl RuntimeStatus {
    pub fn is_running(&self) -> bool {
        matches!(self, Self::Running)
    }

    pub fn is_fatal(&self) -> bool {
        matches!(self, Self::Fatal { .. })
    }
}

/// 前端可见的一条纸条。
///
/// 时间以 RFC3339 字符串下发，避免把后端时间类型泄漏到 TS 边界。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SlipView {
    pub id: String,
    pub text: String,
    pub done: bool,
    /// 有效归档 = `done && archived_at.is_some()`（v11 §1.8）。
    pub archived: bool,
    /// 定稿标记：正文不可改，但完成与归档照常。
    pub locked: bool,
    pub created_at: String,
    pub done_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthView {
    pub writable: bool,
    pub warnings: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum RecoveryView {
    Normal,
    Isolated { corrupt_path: String },
}

/// 一次完整发布。`ui_rev` 单调递增，前端据此丢弃过期快照。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UiSnapshot {
    pub content_rev: u64,
    pub ui_rev: u64,
    pub slips: Vec<SlipView>,
    pub health: HealthView,
    pub needs_reload: bool,
    pub recovery: RecoveryView,
    pub diagnostics: Vec<String>,
    pub runtime: RuntimeStatus,
}

/// 命令应答。`ui_rev` 只在已生成可信 UI 状态时提供。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Ack {
    pub request_id: String,
    pub result: AckResult,
    pub ui_rev: Option<u64>,
    pub error: Option<AckError>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AckResult {
    Applied,
    Rejected,
    Conflict,
    NeedsReload,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AckError {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryError(pub String);
