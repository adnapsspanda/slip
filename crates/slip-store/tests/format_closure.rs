//! 格式闭包：正文经编码进入行、再解析回来必须**逐字符一致**。
//!
//! `encode` 只转义 `&` 与 `<`，`decode` 必须按「先 `&lt;` 后 `&amp;`」的顺序还原。
//! 这个顺序是对的，但极其容易被后来人"整理"成一条 replace 链——那样 `&amp;lt;`
//! 会被解成 `<` 而不是 `&lt;`，**正文静默变形且没有任何报错**。下面把边界逐个钉住。
//!
//! 另外钉住一条更强的性质：对刚写出的文档，`parse(render(doc)) == doc`，
//! 即 render 与 parse 在"我们写过的字节"上互为逆。

use slip_domain::{Mutation, Patch, Slip, SlipId};
use slip_store::doc::{apply, parse, render};
use time::OffsetDateTime;

fn slip(text: &str) -> Slip {
    Slip {
        id: SlipId::new_v7(),
        text: text.to_owned(),
        done: false,
        created_at: OffsetDateTime::now_utc(),
        done_at: None,
        archived_at: None,
        locked: false,
    }
}

/// 追加一条 → 渲染 → 重新解析，返回读回的正文与原始字节。
fn write_then_read(text: &str) -> (String, Vec<u8>) {
    let empty = parse(b"");
    assert!(
        empty.problems.is_empty(),
        "空文档不该有问题：{:?}",
        empty.problems
    );
    let (doc, changed) = apply(&empty.document, &[Mutation::Append(slip(text))]).expect("append");
    assert!(changed, "追加必然改变文档：{text:?}");
    let bytes = render(&doc);
    let reparsed = parse(&bytes);
    assert!(
        reparsed.problems.is_empty(),
        "重新解析出现 problems（{text:?}）：{:?}",
        reparsed.problems
    );
    assert_eq!(reparsed.document.records.len(), 1, "记录数不对：{text:?}");
    (reparsed.document.records[0].slip.text.clone(), bytes)
}

fn assert_round_trip(text: &str) {
    let (read_back, bytes) = write_then_read(text);
    assert_eq!(
        read_back, text,
        "正文没有原样读回：写 {text:?} 得 {read_back:?}"
    );

    // 更强的一条：对刚写出的字节，parse 再 render 必须是恒等。
    let again = render(&parse(&bytes).document);
    assert_eq!(again, bytes, "parse(render) 不是恒等：{text:?}");
}

#[test]
fn entity_escapes_round_trip() {
    // `&` 与 `<` 会被转义；`decode` 的替换顺序决定这些能否还原。
    for text in [
        "a & b",
        "a < b",
        "&amp;",
        "&lt;",
        "&amp;lt;",
        "<< && >>",
        "a &lt; b &amp; c",
        "结尾一个 &",
    ] {
        assert_round_trip(text);
    }
}

#[test]
fn text_that_looks_like_a_marker_round_trips() {
    // 正文里出现“像 marker”的片段时，解析靠 find_last 定位真正的 marker。
    let id = SlipId::new_v7().to_canonical_string();
    for text in [
        format!("伪记录 <!-- slip:v1;id={id};created=2026-11-10T00:00:00Z -->"),
        String::from("未闭合注释 <!-- 后面没有收尾"),
        String::from("箭头 --> 与 <!-- 混在一起"),
        String::from("- [x] 看起来像一条已完成记录"),
    ] {
        assert_round_trip(&text);
    }
}

#[test]
fn zero_width_and_emoji_round_trip() {
    // 零宽连接符、词连接符、变体选择符、组合附加符——都不该被吞掉或改写。
    for text in [
        "👨‍👩‍👧 家庭",
        "\u{200D}孤立连接符",
        "\u{FEFF}词连接符在开头",
        "❤\u{FE0F} 变体选择符",
        "e\u{0301} 组合重音",
        "عربي RTL 文本",
    ] {
        assert_round_trip(text);
    }
}

#[test]
fn control_like_characters_that_are_not_whitespace_round_trip() {
    // 这些不是 White_Space，因此不会被 trim 掉，必须原样保留。
    for text in [
        "\u{00A0}不换行空格在开头",
        "a\u{2007}b 数字宽空格",
        "\u{2028}行分隔符",
    ] {
        // U+2028 是 White_Space，normalize_text 会把它当空白处理；
        // 这里只断言"经领域层规范化之后再写出的内容能原样读回"。
        let normalized = slip_domain::normalize_text(text).expect("应通过规范化");
        assert_round_trip(&normalized);
    }
}

#[test]
fn long_text_round_trips() {
    // 接近码点上限的正文：保证行长度处理没有隐含截断。
    let text = "字".repeat(slip_domain::MAX_TEXT_CODEPOINTS);
    assert_round_trip(&text);
}

// ---------------------------------------------------------------- BOM（§4.1）

/// BOM 必须**识别得出来、并按原样写回**：一份带 BOM 的文件在没有任何改动时，
/// `parse` → `render` 必须逐字节相同，否则"外部编辑器打开再保存"会莫名少三个字节。
///
/// 写这条之前 BOM 处理**没有任何测试**：把 `render` 里的 BOM 分支删掉也不会红。
#[test]
fn bom_is_detected_and_written_back_byte_identically() {
    let mut bytes = vec![0xEF, 0xBB, 0xBF];
    bytes.extend_from_slice(b"- [ ] one <!-- slip:v1;id=01a1c7e0-0000-7000-8000-000000000001;created=2026-11-10T00:00:00Z -->\n");

    let parsed = parse(&bytes);
    assert!(parsed.document.bom, "带 BOM 的字节必须被识别为 bom=true");

    let rendered = render(&parsed.document);
    assert_eq!(
        rendered, bytes,
        "无改动时 parse→render 必须逐字节相同（含 BOM）"
    );
}

/// 真正提交时统一输出**无 BOM**（§4.1）：这是刻意的归一化，
/// 不是遗漏——所以要有测试把它钉住，免得有人"顺手"改成保留 BOM。
#[test]
fn a_real_change_drops_the_bom() {
    let mut bytes = vec![0xEF, 0xBB, 0xBF];
    bytes.extend_from_slice(b"- [ ] one <!-- slip:v1;id=01a1c7e0-0000-7000-8000-000000000001;created=2026-11-10T00:00:00Z -->\n");

    let parsed = parse(&bytes);
    let slip = parsed.document.records[0].slip.clone();

    let (next, changed) = apply(
        &parsed.document,
        &[Mutation::Patch {
            id: slip.id,
            patch: Patch::SetText(String::from("two")),
        }],
    )
    .expect("apply");

    assert!(changed, "改正文应当算真的改变");
    assert!(!next.bom, "真正提交后应输出无 BOM");

    let rendered = render(&next);
    assert_ne!(rendered.first(), Some(&0xEF), "输出不应再带 BOM 首字节");
}
