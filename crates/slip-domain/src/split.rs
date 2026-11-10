//! 多行捕获拆条（v11 §1.6）。
//!
//! 关键口径：**2000 码点是单条上限，不是整段粘贴文本的上限。**
//! 一段 5000 字的粘贴如果分成 10 行，每行 500 字，是合法的。

use time::OffsetDateTime;

use crate::error::DomainError;
use crate::model::Draft;
use crate::text::normalize_text;

/// 单次捕获的条数上限。
pub const MAX_LINES: usize = 100;

/// 把一次捕获的原始文本拆成若干草稿。
///
/// - 按 `\r\n | \r | \n` 切分；
/// - 每行去首尾空白后为空则丢弃；
/// - 每行走 [`normalize_text`] 的完整校验；
/// - 去空白后的条数为 `1..=MAX_LINES`，超出**整批拒绝**（不截断）；
/// - 全部为空 → [`DomainError::EmptyCapture`]；
/// - 同批所有草稿共用 `occurred_at`。
pub fn split_drafts(raw: &str, occurred_at: OffsetDateTime) -> Result<Vec<Draft>, DomainError> {
    let mut drafts = Vec::new();

    for line in raw.split(['\r', '\n']) {
        if line.trim().is_empty() {
            continue;
        }
        // normalize_text 自己会 trim，这里不预 trim，保持单一裁决点。
        let text = normalize_text(line)?;
        drafts.push(Draft { text, occurred_at });

        // 先加后判：能准确报出"至少超到几条"。
        if drafts.len() > MAX_LINES {
            return Err(DomainError::TooManyLines {
                actual: drafts.len(),
                max: MAX_LINES,
            });
        }
    }

    if drafts.is_empty() {
        return Err(DomainError::EmptyCapture);
    }
    Ok(drafts)
}

#[cfg(test)]
mod tests {
    use time::macros::datetime;

    use super::*;

    fn at() -> OffsetDateTime {
        datetime!(2026-11-10 11:49:14 +08:00)
    }

    fn texts(raw: &str) -> Result<Vec<String>, DomainError> {
        split_drafts(raw, at()).map(|v| v.into_iter().map(|d| d.text).collect())
    }

    #[test]
    fn splits_lf() {
        assert_eq!(texts("a\nb\nc").unwrap(), vec!["a", "b", "c"]);
    }

    #[test]
    fn splits_crlf() {
        assert_eq!(texts("a\r\nb\r\nc").unwrap(), vec!["a", "b", "c"]);
    }

    #[test]
    fn splits_cr() {
        assert_eq!(texts("a\rb\rc").unwrap(), vec!["a", "b", "c"]);
    }

    #[test]
    fn drops_blank_lines() {
        assert_eq!(texts("a\n\n\n  \n\t\nb\n").unwrap(), vec!["a", "b"]);
    }

    #[test]
    fn trims_each_line() {
        assert_eq!(
            texts("  买发票  \n\t问 X 要收据\t").unwrap(),
            vec!["买发票", "问 X 要收据"]
        );
    }

    #[test]
    fn rejects_wholly_blank_capture() {
        assert_eq!(texts("   \n\n\t\r\n"), Err(DomainError::EmptyCapture));
        assert_eq!(texts(""), Err(DomainError::EmptyCapture));
    }

    #[test]
    fn shares_one_timestamp_across_batch() {
        let drafts = split_drafts("a\nb", at()).unwrap();
        assert!(drafts.iter().all(|d| d.occurred_at == at()));
    }

    #[test]
    fn accepts_exactly_max_lines() {
        let raw = (0..MAX_LINES)
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(split_drafts(&raw, at()).unwrap().len(), MAX_LINES);
    }

    #[test]
    fn rejects_over_max_lines_without_truncating() {
        let raw = (0..=MAX_LINES)
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(
            split_drafts(&raw, at()),
            Err(DomainError::TooManyLines {
                actual: MAX_LINES + 1,
                max: MAX_LINES,
            })
        );
    }

    #[test]
    fn blank_lines_do_not_count_toward_limit() {
        // 200 行但一半是空白：去空白后正好 100 条，应通过。
        let mut raw = String::new();
        for i in 0..MAX_LINES {
            raw.push_str(&format!("{i}\n\n"));
        }
        assert_eq!(split_drafts(&raw, at()).unwrap().len(), MAX_LINES);
    }

    #[test]
    fn limit_is_per_line_not_per_paste() {
        // 每行 1500 码点、共 5 行 = 7500 码点总量，但每条都合法。
        let line = "a".repeat(1500);
        let raw = vec![line.clone(); 5].join("\n");
        let drafts = split_drafts(&raw, at()).unwrap();
        assert_eq!(drafts.len(), 5);
        assert!(drafts.iter().all(|d| d.text.chars().count() == 1500));
    }

    #[test]
    fn rejects_when_a_single_line_is_too_long() {
        let raw = format!("ok\n{}", "a".repeat(2001));
        assert!(matches!(
            split_drafts(&raw, at()),
            Err(DomainError::TextTooLong { .. })
        ));
    }
}
