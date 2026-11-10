//! 文本规范化与长度口径（v11 §1.4、§1.6）。
//!
//! 长度单位是 **Unicode 码点**（Rust `chars().count()`，前端 `[...s].length`），
//! 不是字节数，也不是 UTF-16 码元。两边单位不一致会造成"前端放行、后端拒绝"。
//!
//! 裁决点只有这里。前端 `Capture.svelte` 的同口径计数只用来**提前提示**
//! （说清是第几行、超了多少），不作数：它可能被输入法组字或粘贴的换行差异
//! 落在后面，最终仍以本模块的判定为准。

use crate::error::DomainError;

/// 单条纸条的码点上限。
pub const MAX_TEXT_CODEPOINTS: usize = 2000;

/// 码点计数。与前端 `[...text].length` 同口径。
pub fn codepoint_len(text: &str) -> usize {
    text.chars().count()
}

/// 规范化单条纸条正文。
///
/// 规则（严格按顺序）：
/// 1. 去首尾 Unicode White_Space（等价于 Rust `str::trim`）；
/// 2. 结果为空 → [`DomainError::EmptyText`]；
/// 3. 结果仍含 CR/LF → [`DomainError::TextContainsNewline`]
///    （首尾换行会被第 1 步吸收，中间换行则拒绝）；
/// 4. 码点数为 `1..=MAX_TEXT_CODEPOINTS`，超出 → [`DomainError::TextTooLong`]；
/// 5. 不合并内部空白，不做 Unicode 归一化（NFC/NFD 原样保留）。
pub fn normalize_text(raw: &str) -> Result<String, DomainError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(DomainError::EmptyText);
    }
    if trimmed.contains('\r') || trimmed.contains('\n') {
        return Err(DomainError::TextContainsNewline);
    }
    let len = codepoint_len(trimmed);
    if len > MAX_TEXT_CODEPOINTS {
        return Err(DomainError::TextTooLong {
            actual: len,
            max: MAX_TEXT_CODEPOINTS,
        });
    }
    Ok(trimmed.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trims_surrounding_whitespace() {
        assert_eq!(normalize_text("  \t 买发票 \u{3000} ").unwrap(), "买发票");
    }

    #[test]
    fn rejects_empty_and_whitespace_only() {
        assert_eq!(normalize_text(""), Err(DomainError::EmptyText));
        assert_eq!(normalize_text("   \t\r\n"), Err(DomainError::EmptyText));
    }

    #[test]
    fn keeps_internal_whitespace() {
        assert_eq!(normalize_text("  a  b  ").unwrap(), "a  b");
    }

    #[test]
    fn rejects_interior_newline() {
        assert_eq!(
            normalize_text("a\nb"),
            Err(DomainError::TextContainsNewline)
        );
        assert_eq!(
            normalize_text("a\rb"),
            Err(DomainError::TextContainsNewline)
        );
    }

    #[test]
    fn leading_trailing_newline_is_trimmed_not_rejected() {
        assert_eq!(normalize_text("\n 买发票 \n").unwrap(), "买发票");
    }

    #[test]
    fn counts_code_points_not_bytes() {
        // 每个汉字 1 码点 / 3 字节；"买发票" 三个码点。
        assert_eq!(codepoint_len("买发票"), 3);
        // 单个 emoji 是 1 码点（不是 UTF-16 的 2 码元）。
        assert_eq!(codepoint_len("😀"), 1);
        // ZWJ 家庭序列是 5 个码点，不做字形簇合并。
        assert_eq!(codepoint_len("👨‍👩‍👧"), 5);
    }

    #[test]
    fn accepts_exactly_the_limit() {
        let text = "a".repeat(MAX_TEXT_CODEPOINTS);
        assert_eq!(normalize_text(&text).unwrap().chars().count(), 2000);
    }

    #[test]
    fn rejects_one_over_the_limit() {
        let text = "a".repeat(MAX_TEXT_CODEPOINTS + 1);
        assert_eq!(
            normalize_text(&text),
            Err(DomainError::TextTooLong {
                actual: MAX_TEXT_CODEPOINTS + 1,
                max: MAX_TEXT_CODEPOINTS,
            })
        );
    }

    #[test]
    fn limit_is_measured_after_trimming() {
        let text = format!("  {}  ", "a".repeat(MAX_TEXT_CODEPOINTS));
        // 去空白后正好 2000 码点，应通过。
        assert!(normalize_text(&text).is_ok());
    }

    #[test]
    fn does_not_normalize_unicode() {
        // "é" 有合成与分解两种表示，都要原样保留。
        let composed = "\u{00E9}";
        let decomposed = "e\u{0301}";
        assert_eq!(normalize_text(composed).unwrap(), composed);
        assert_eq!(normalize_text(decomposed).unwrap(), decomposed);
    }

    #[test]
    fn is_idempotent() {
        let once = normalize_text("  a b  ").unwrap();
        let twice = normalize_text(&once).unwrap();
        assert_eq!(once, twice);
    }
}
