use std::fmt;

/// 领域错误。
///
/// 这里只表达"领域规则不满足"，不表达 I/O 失败或布局问题。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DomainError {
    /// 文本为空，或去首尾空白后为空。
    EmptyText,
    /// 文本超过码点上限。
    TextTooLong { actual: usize, max: usize },
    /// 文本内含 CR/LF（单条纸条不允许换行）。
    TextContainsNewline,
    /// 文本不是 `normalize_text` 的规范形式（v11 §1.4）。
    TextNotNormalized,
    /// ID 非 nil、版本非 7，或不是小写连字符规范形式。
    InvalidId,
    /// 时间无法用文件实际使用的 RFC3339 格式往返（v11 §1.4 第三项）。
    TimestampNotSerializable,
    /// 捕获内容拆条后为空。
    EmptyCapture,
    /// 单次捕获条数超限。
    TooManyLines { actual: usize, max: usize },
    /// 未完成的纸条不能归档。
    ArchiveRequiresDone,
    /// 纸条已定稿（锁定），正文不可再改。
    SlipLocked,
}

impl fmt::Display for DomainError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyText => write!(f, "text is empty after trimming"),
            Self::TextTooLong { actual, max } => {
                write!(f, "text has {actual} code points, limit is {max}")
            }
            Self::TextContainsNewline => write!(f, "text must not contain CR or LF"),
            Self::TextNotNormalized => write!(f, "text is not in normalized form"),
            Self::InvalidId => write!(f, "invalid slip id"),
            Self::TimestampNotSerializable => {
                write!(f, "timestamp cannot round-trip through RFC3339")
            }
            Self::EmptyCapture => write!(f, "capture produced no slips"),
            Self::TooManyLines { actual, max } => {
                write!(f, "capture has {actual} slips, limit is {max}")
            }
            Self::ArchiveRequiresDone => write!(f, "cannot archive a slip that is not done"),
            Self::SlipLocked => write!(f, "slip is locked; unlock it before editing text"),
        }
    }
}

impl std::error::Error for DomainError {}
