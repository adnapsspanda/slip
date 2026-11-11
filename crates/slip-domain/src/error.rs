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
            // 这些句子会**原样出现在用户的提示区**（经 ACK 的 message 字段），
            // 所以必须是中文，而且要说清"该怎么办"，不是复述错误码。
            Self::EmptyText => write!(f, "内容为空（去掉首尾空白后什么也不剩）"),
            Self::TextTooLong { actual, max } => {
                write!(f, "太长了：{actual} 个字，上限 {max} 个字")
            }
            Self::TextContainsNewline => write!(f, "单条内容不能包含换行"),
            Self::TextNotNormalized => write!(f, "内容不是规范化的 Unicode 形式"),
            Self::InvalidId => write!(f, "纸条编号不合法"),
            Self::TimestampNotSerializable => {
                write!(f, "时间戳无法按 RFC3339 精确往返")
            }
            Self::EmptyCapture => write!(f, "要收录的内容是空的"),
            Self::TooManyLines { actual, max } => {
                write!(f, "一次最多收录 {max} 条，现在是 {actual} 条")
            }
            Self::ArchiveRequiresDone => write!(f, "未完成的纸条不能归档"),
            Self::SlipLocked => write!(f, "纸条已锁定，先解锁才能改正文"),
        }
    }
}

impl std::error::Error for DomainError {}
