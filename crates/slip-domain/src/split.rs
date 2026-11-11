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

/// 剥掉行首的列表标记：`- ` / `* ` / `+ ` / `1. ` / `1) `，以及紧随其后的 checkbox。
///
/// 粘贴 Markdown 待办清单是常见操作，不剥的话会写出 `- [ ] - 买发票` 这种双重标记。
///
/// 三条刻意的保守规则：
/// 1. 分隔符后**必须跟空格**——`1.5 倍`、`-5 度`、`-- 注意` 都不是列表前缀；
/// 2. 只剥**一层**；`- - 嵌套` 得到 `- 嵌套`，不做多轮；
/// 3. 有序列表的数字最多 3 位，`2026.11.11` 这种日期不会被当成序号。
fn strip_list_prefix(line: &str) -> &str {
    let s = line.trim_start();

    let after_marker = s
        .strip_prefix("- ")
        .or_else(|| s.strip_prefix("* "))
        .or_else(|| s.strip_prefix("+ "))
        .or_else(|| strip_ordered_marker(s));

    let Some(rest) = after_marker else {
        return s;
    };

    // checkbox 只在列表标记之后才剥：裸的 `[ ]` 更像是正文里的方括号。
    for checkbox in ["[ ] ", "[x] ", "[X] "] {
        if let Some(body) = rest.strip_prefix(checkbox) {
            return body;
        }
    }
    rest
}

/// `1. ` 与 `1) ` 两种有序列表前缀。数字 1~3 位，且后面必须有空格。
fn strip_ordered_marker(s: &str) -> Option<&str> {
    let digits = s.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits == 0 || digits > 3 {
        return None;
    }
    let rest = &s[digits..];
    rest.strip_prefix(". ").or_else(|| rest.strip_prefix(") "))
}

/// 把一次捕获的原始文本拆成若干草稿。
///
/// - 按 `\r\n | \r | \n` 切分；
/// - 每行先剥掉列表前缀（见 [`strip_list_prefix`]），去首尾空白后为空则丢弃；
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
        // 只在下标前缀之后还剩东西时才继续：`- ` 这种空列表项按空行处理。
        let stripped = strip_list_prefix(line);
        if stripped.trim().is_empty() {
            continue;
        }
        // normalize_text 自己会 trim，这里不预 trim，保持单一裁决点。
        let text = normalize_text(stripped)?;
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

    // ---------------- 列表前缀清洗 ----------------

    #[test]
    fn strips_common_list_prefixes() {
        // 粘贴 Markdown 清单是常见操作：不剥会写出 `- [ ] - 买发票` 这种双重标记。
        assert_eq!(
            texts("- 买发票\n* 问 X 要收据\n+ third").unwrap(),
            vec!["买发票", "问 X 要收据", "third"]
        );
        assert_eq!(texts("1. 第一\n2) 第二").unwrap(), vec!["第一", "第二"]);
    }

    #[test]
    fn strips_checkbox_only_after_a_list_marker() {
        assert_eq!(
            texts("- [ ] 待办\n- [x] 已完成").unwrap(),
            vec!["待办", "已完成"]
        );
        // 裸的 `[ ]` 更像正文里的方括号，不剥。
        assert_eq!(texts("[ ] 不是列表").unwrap(), vec!["[ ] 不是列表"]);
    }

    #[test]
    fn keeps_things_that_only_look_like_prefixes() {
        for line in [
            "1.5 倍",          // 数字后是小数点，不是 "点+空格"
            "-5 度",           // 没有空格分隔
            "-- 注意",         // 不是单层标记
            "2026.11.11 发布", // 4 位数字不算序号
            "**重点**",        // 与列表无关
        ] {
            assert_eq!(texts(line).unwrap(), vec![line], "不该剥：{line}");
        }
    }

    #[test]
    fn strips_only_one_level() {
        assert_eq!(texts("- - 嵌套").unwrap(), vec!["- 嵌套"]);
    }

    #[test]
    fn empty_list_item_is_dropped_like_a_blank_line() {
        // 「- 」本身没有内容，按空行处理；一个只剩空列表项的捕获等于空捕获。
        assert_eq!(texts("a\n- \nb").unwrap(), vec!["a", "b"]);
        assert_eq!(texts("- "), Err(DomainError::EmptyCapture));
    }

    #[test]
    fn stripped_lines_still_respect_the_per_line_limit() {
        // 剥前缀之后才判长度：`- ` 不该占掉两个码点配额。
        let long = format!("- {}", "a".repeat(crate::MAX_TEXT_CODEPOINTS));
        assert!(split_drafts(&long, at()).is_ok());
    }
}
