//! 从纸条正文里提取 `#标签`。
//!
//! **不引入 regex 库**：标签的边界规则很简单（按空白切分 + 前缀判定），用正则反而会
//! 变成第二个需要维护的文法；而且正则引擎在这个位置没有性能优势。
//!
//! 设计上的一条硬约束：**这个函数只读不写**。标签是正文的派生视图——它不改写 `text`、
//! 不参与落盘、不进入 marker。否则就会破坏"未修改行逐字节保留"，也会让搜索与导出
//! 看到被"加工"过的内容。

/// 可以作为标签结尾、应当被剥掉的标点（中英文常见的那几个）。
///
/// 这些字符**只在末尾**被剥掉：`#前#后` 这种中间带 `#` 的直接判定为不是标签（见下）。
fn is_trailing_punct(c: char) -> bool {
    matches!(
        c,
        ',' | '.'
            | ';'
            | ':'
            | '!'
            | '?'
            | ')'
            | ']'
            | '}'
            | '"'
            | '\''
            | '。'
            | '，'
            | '、'
            | '；'
            | '：'
            | '！'
            | '？'
            | '）'
            | '】'
            | '》'
            | '」'
            | '』'
            | '…'
    )
}

/// 标签前面可能出现的开括号——`（#工作）` 这种也该认出标签。
fn is_leading_wrapper(c: char) -> bool {
    matches!(
        c,
        '(' | '[' | '{' | '（' | '【' | '《' | '「' | '『' | '"' | '\''
    )
}

/// 从一个条目的正文里提取标签，按**首次出现顺序**去重。
///
/// 规则（逐条都对应一条测试）：
/// 1. 按空白切成 token——标签不能跨空格；
/// 2. token 去掉开头的括号后，必须以 `#` 打头；
/// 3. `#` 之后不能为空、不能再含 `#`、不能含路径分隔符；
/// 4. 末尾的中英文标点会被剥掉（`#工作，` → `工作`）。
///
/// **URL 里的 `#anchor` 天然被排除**：`https://a.com#b` 这个 token 不是以 `#` 开头，
/// 规则 2 就把它挡掉了。不需要额外判断"是不是 URL"。
pub fn extract_tags(text: &str) -> Vec<String> {
    let mut tags: Vec<String> = Vec::new();
    for raw in text.split_whitespace() {
        // 剥掉开头的括号/引号：`（#工作）` 里 `#` 前面还有个全角括号。
        let mut token = raw;
        while let Some(rest) = token.strip_prefix(is_leading_wrapper) {
            token = rest;
        }
        // 剥掉末尾标点。
        token = token.trim_end_matches(is_trailing_punct);

        let Some(rest) = token.strip_prefix('#') else {
            continue;
        };
        // 空标签（只有 `#`）、多重 `#`（`#a#b`）、路径形式（`#a/b`）都不算。
        if rest.is_empty() || rest.contains('#') {
            continue;
        }
        if rest.contains('/') || rest.contains('\\') {
            continue;
        }
        // 纯标点的"标签"没有意义（`#!!`）。
        if !rest.chars().any(|c| c.is_alphanumeric()) {
            continue;
        }
        if !tags.iter().any(|existing| existing == rest) {
            tags.push(rest.to_owned());
        }
    }
    tags
}

#[cfg(test)]
mod tests {
    use super::extract_tags;

    fn tags(text: &str) -> Vec<String> {
        extract_tags(text)
    }

    #[test]
    fn extracts_simple_english_tags() {
        assert_eq!(tags("#work"), vec!["work"]);
        assert_eq!(tags("#work #life"), vec!["work", "life"]);
        assert_eq!(tags("今天要写 #work 的报告"), vec!["work"]);
    }

    #[test]
    fn extracts_chinese_tags() {
        assert_eq!(tags("#工作"), vec!["工作"]);
        assert_eq!(tags("做点事 #工作 #生活"), vec!["工作", "生活"]);
        // 中英混合的标签名也照收
        assert_eq!(tags("#待办2"), vec!["待办2"]);
    }

    #[test]
    fn url_anchor_is_not_a_tag() {
        // 这条是矩阵明确点名的：URL 里的 #anchor 不能被当成标签。
        assert_eq!(tags("https://a.com#b"), Vec::<String>::new());
        assert_eq!(
            tags("见 https://example.com/page#section 这里有说明"),
            Vec::<String>::new()
        );
        // 但同一句里的真标签仍然要认出来
        assert_eq!(tags("https://a.com#b 另外 #work"), vec!["work"]);
    }

    #[test]
    fn strips_trailing_punctuation() {
        assert_eq!(tags("#工作，"), vec!["工作"]);
        assert_eq!(tags("#work."), vec!["work"]);
        assert_eq!(tags("#work!?"), vec!["work"]);
        assert_eq!(tags("#工作。"), vec!["工作"]);
        assert_eq!(tags("#work）"), vec!["work"]);
    }

    #[test]
    fn handles_wrapping_brackets() {
        assert_eq!(tags("（#工作）"), vec!["工作"]);
        assert_eq!(tags("(#work)"), vec!["work"]);
        assert_eq!(tags("「#工作」"), vec!["工作"]);
    }

    #[test]
    fn rejects_degenerate_tokens() {
        assert_eq!(tags("#"), Vec::<String>::new());
        assert_eq!(tags("# "), Vec::<String>::new());
        assert_eq!(tags("#a#b"), Vec::<String>::new());
        assert_eq!(tags("#a/b"), Vec::<String>::new());
        assert_eq!(tags("#!!"), Vec::<String>::new());
        assert_eq!(tags("没有标签"), Vec::<String>::new());
        // `#` 在中间不算标签（正是 URL 锚点那种形态）
        assert_eq!(tags("a#b"), Vec::<String>::new());
        // Markdown 标题前缀不该被当成标签
        assert_eq!(tags("# 标题"), Vec::<String>::new());
    }

    #[test]
    fn deduplicates_preserving_first_appearance_order() {
        assert_eq!(tags("#b #a #b #c #a"), vec!["b", "a", "c"]);
    }

    #[test]
    fn keeps_tag_names_case_sensitive() {
        // 标签是用户自己起的名字，不替他做大小写归一——那会让 #Work 和 #work 莫名合并。
        assert_eq!(tags("#Work #work"), vec!["Work", "work"]);
    }

    #[test]
    fn does_not_touch_the_input_text() {
        // 派生视图：函数只读，调用前后正文不变（这里用不可变绑定表达契约）。
        let text = "#工作 今天要把报告写完";
        let before = text.to_owned();
        let _ = tags(text);
        assert_eq!(text, before);
    }
}
