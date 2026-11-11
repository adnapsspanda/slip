//! Slip 领域模型。
//!
//! 只包含类型、纯函数与状态转换：零 I/O、零 `tauri`、零平台依赖。
//! 对应 v11 §1。所有校验口径以文档为准，不以调用方的便利为准。

mod error;
mod id;
mod model;
mod split;
mod state;
mod tags;
mod text;

pub use error::DomainError;
pub use id::SlipId;
pub use model::{ContentRev, Draft, Mutation, Patch, Slip, Transition};
pub use split::{MAX_LINES, split_drafts};
pub use state::{apply_patch, validate_slip, validate_time};
pub use tags::extract_tags;
pub use text::{
    MAX_TEXT_CODEPOINTS, codepoint_len, native_newline, normalize_newlines, normalize_text,
    with_newline_style,
};
