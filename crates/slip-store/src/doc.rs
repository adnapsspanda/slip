//! Markdown 真源的解析、渲染与局部替换。
//!
//! 三条不可协商的性质：
//! 1. **未修改行逐字节保留**——原始字节是唯一真源，解析不改文件；
//! 2. **候选文档必须由原始行数组局部替换构造**，禁止全文重新序列化；
//! 3. 回读闭包必须同时验证"记录相等"和"未修改行字节相等"。

use slip_domain::{
    DomainError, Mutation, Slip, SlipId, Transition, apply_patch, normalize_text, validate_time,
};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// 记录 marker 的固定起始片段（含前面的分隔空格）。
const MARKER_NEEDLE: &[u8] = b" <!-- slip:";
const MARKER_PREFIX: &str = " <!-- ";
const MARKER_SUFFIX: &str = " -->";
const HEADER: &str = "slip:v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Terminator {
    Lf,
    CrLf,
    Cr,
    /// 文件最后一行没有终止符。
    None,
}

impl Terminator {
    pub fn as_bytes(self) -> &'static [u8] {
        match self {
            Self::Lf => b"\n",
            Self::CrLf => b"\r\n",
            Self::Cr => b"\r",
            Self::None => b"",
        }
    }
}

/// 记录候选行不合法的原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvalidReason {
    /// 记录候选行含非法 UTF-8。
    NonUtf8,
    /// checkbox 不是 `- [ ]` / `- [x]` / `- [X]`。
    BadCheckbox,
    /// 头部不是 `slip:v1`。
    UnsupportedVersion,
    /// marker 结构不完整或不合法。
    BadMarker,
    /// kv 片段不满足 `key=value` 文法。
    BadKeyValue,
    /// 缺少必填键。
    MissingField,
    /// 已知键重复。
    DuplicateKey,
    /// `id` 不是合法 UUIDv7 小写连字符形式。
    BadId,
    /// 时间无法按 RFC3339 往返。
    BadTime,
    /// 文本去空白后为空。
    EmptyText,
    /// 文本超长。
    TextTooLong,
}

/// 未知键片段。**必须逐字保留其原始写法与相对顺序。**
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtraField {
    pub key: String,
    pub value: String,
    /// 原始片段文本（`key=value`），用于未修改时逐字输出。
    pub raw: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Record {
    pub slip: Slip,
    pub extras: Vec<ExtraField>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    /// 合法记录，`index` 指向 [`Document::records`]。
    Record { index: usize },
    /// 非记录行：标题、说明、代码块、无 marker 的 todo、非 UTF-8 普通行。
    Other,
    /// 记录候选但结构不合法。
    Invalid { reason: InvalidReason },
}

#[derive(Debug, Clone)]
pub struct Line {
    /// 原始字节，**含终止符**。修改该行时整体替换。
    pub raw: Vec<u8>,
    pub terminator: Terminator,
    pub kind: LineKind,
}

#[derive(Debug, Clone)]
pub struct Document {
    /// 文件开头有 BOM。解析视图去除，渲染时按原样写回。
    pub bom: bool,
    pub lines: Vec<Line>,
    pub records: Vec<Record>,
    /// 文件中第一个实际出现的终止符（不按出现次数投票）。
    pub preferred_terminator: Option<Terminator>,
    /// 原文末尾是否有终止符；决定追加后新末行是否带换行。
    pub ends_with_terminator: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    /// 1 基行号。
    pub line: usize,
    pub reason: InvalidReason,
}

#[derive(Debug, Clone)]
pub struct Parsed {
    pub document: Document,
    /// 记录候选行的语法问题。非空 → 整库不可写。
    pub problems: Vec<Problem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplyError {
    SlipNotFound { id: SlipId },
    DuplicateId { id: SlipId },
    Domain(DomainError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoundtripError {
    /// 候选字节回读失败。
    ReparseFailed,
    /// 回读出的记录与预期不一致。
    RecordMismatch { index: usize },
    /// 回读出现重复 ID。
    DuplicateId,
    /// 未被修改的行字节发生了变化——违反"Markdown 是真源"。
    UnmodifiedLineChanged { line: usize },
}

// ---------------------------------------------------------------- 解析

/// 解析整份文件字节。
pub fn parse(bytes: &[u8]) -> Parsed {
    let (bom, body) = match bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        Some(rest) => (true, rest),
        None => (false, bytes),
    };

    let mut lines: Vec<Line> = Vec::new();
    let mut records: Vec<Record> = Vec::new();
    let mut problems: Vec<Problem> = Vec::new();
    let mut preferred_terminator: Option<Terminator> = None;

    let mut start = 0usize;
    let mut i = 0usize;
    while i < body.len() {
        let term = match body[i] {
            b'\r' if i + 1 < body.len() && body[i + 1] == b'\n' => Some((Terminator::CrLf, 2)),
            b'\r' => Some((Terminator::Cr, 1)),
            b'\n' => Some((Terminator::Lf, 1)),
            _ => None,
        };
        let Some((terminator, term_len)) = term else {
            i += 1;
            continue;
        };

        let content_end = i;
        let raw_end = i + term_len;
        let line_no = lines.len() + 1;
        let mut line = build_parsed_line(
            &body[start..raw_end],
            content_end - start,
            terminator,
            line_no,
            &mut records,
            &mut problems,
        );
        line.raw.copy_from_slice(&body[start..raw_end]);
        if preferred_terminator.is_none() {
            preferred_terminator = Some(terminator);
        }
        lines.push(line);

        i = raw_end;
        start = raw_end;
    }

    if start < body.len() {
        let line_no = lines.len() + 1;
        let mut line = build_parsed_line(
            &body[start..],
            body.len() - start,
            Terminator::None,
            line_no,
            &mut records,
            &mut problems,
        );
        line.raw.copy_from_slice(&body[start..]);
        lines.push(line);
    }

    let ends_with_terminator = !body.is_empty() && start == body.len();

    Parsed {
        document: Document {
            bom,
            lines,
            records,
            preferred_terminator,
            ends_with_terminator,
        },
        problems,
    }
}

fn build_parsed_line(
    raw: &[u8],
    content_len: usize,
    terminator: Terminator,
    line_no: usize,
    records: &mut Vec<Record>,
    problems: &mut Vec<Problem>,
) -> Line {
    let content = &raw[..content_len];
    let kind = match classify(content) {
        Ok(Some(record)) => {
            let index = records.len();
            records.push(record);
            LineKind::Record { index }
        }
        Ok(None) => LineKind::Other,
        Err(reason) => {
            problems.push(Problem {
                line: line_no,
                reason,
            });
            LineKind::Invalid { reason }
        }
    };
    Line {
        raw: raw.to_vec(),
        terminator,
        kind,
    }
}

/// 返回 `Ok(None)` 表示不是记录候选，`Ok(Some)` 表示合法记录。
fn classify(content: &[u8]) -> Result<Option<Record>, InvalidReason> {
    if !content.starts_with(b"- [") {
        return Ok(None);
    }
    let Some(pos) = find_last(content, MARKER_NEEDLE) else {
        return Ok(None);
    };

    // 从这里开始就是"记录候选"：任何结构问题都必须报错，不能静默当普通行。
    let text = std::str::from_utf8(content).map_err(|_| InvalidReason::NonUtf8)?;
    let bytes = text.as_bytes();
    if bytes.len() < 6 {
        return Err(InvalidReason::BadCheckbox);
    }
    if !matches!(bytes[3], b' ' | b'x' | b'X') || bytes[4] != b']' || bytes[5] != b' ' {
        return Err(InvalidReason::BadCheckbox);
    }
    let done = bytes[3] != b' ';

    // needle 带前导空格，正文为空时 `pos == 5`（正好落在 checkbox 后的空格上）。
    // 先挡掉再切片，否则 `&text[6..5]` 会 panic，把坏文件升级成进程崩溃。
    if pos < 6 {
        return Err(InvalidReason::EmptyText);
    }

    // `pos` 落在 ASCII 空格上，必然是字符边界。
    let text_part = &text[6..pos];
    let marker = &text[pos..];

    if !marker.starts_with(MARKER_PREFIX) || !marker.ends_with(MARKER_SUFFIX) {
        return Err(InvalidReason::BadMarker);
    }
    let inner = &marker[MARKER_PREFIX.len()..marker.len() - MARKER_SUFFIX.len()];

    let kv_part = if inner == HEADER {
        ""
    } else if let Some(rest) = inner.strip_prefix(HEADER).and_then(|r| r.strip_prefix(';')) {
        rest
    } else if inner.starts_with("slip:") {
        return Err(InvalidReason::UnsupportedVersion);
    } else {
        return Err(InvalidReason::BadMarker);
    };

    let mut raw_id: Option<&str> = None;
    let mut raw_created: Option<&str> = None;
    let mut raw_done_at: Option<&str> = None;
    let mut raw_archived_at: Option<&str> = None;
    let mut raw_locked: Option<bool> = None;
    let mut extras: Vec<ExtraField> = Vec::new();

    if !kv_part.is_empty() {
        for segment in kv_part.split(';') {
            let Some(eq) = segment.find('=') else {
                return Err(InvalidReason::BadKeyValue);
            };
            let (key, value) = (&segment[..eq], &segment[eq + 1..]);
            if !valid_key(key) || !valid_value(value) {
                return Err(InvalidReason::BadKeyValue);
            }
            match key {
                "id" => {
                    if raw_id.replace(value).is_some() {
                        return Err(InvalidReason::DuplicateKey);
                    }
                }
                "created" => {
                    if raw_created.replace(value).is_some() {
                        return Err(InvalidReason::DuplicateKey);
                    }
                }
                "done_at" => {
                    if raw_done_at.replace(value).is_some() {
                        return Err(InvalidReason::DuplicateKey);
                    }
                }
                "archived_at" => {
                    if raw_archived_at.replace(value).is_some() {
                        return Err(InvalidReason::DuplicateKey);
                    }
                }
                "locked" => {
                    if raw_locked.is_some() {
                        return Err(InvalidReason::DuplicateKey);
                    }
                    // 只认 `0`/`1`：别的写法说明有人手改错了，宁可报错也不要猜。
                    raw_locked = Some(match value {
                        "1" => true,
                        "0" => false,
                        _ => return Err(InvalidReason::BadKeyValue),
                    });
                }
                _ => extras.push(ExtraField {
                    key: key.to_owned(),
                    value: value.to_owned(),
                    raw: segment.to_owned(),
                }),
            }
        }
    }

    let raw_id = raw_id.ok_or(InvalidReason::MissingField)?;
    let raw_created = raw_created.ok_or(InvalidReason::MissingField)?;

    let id = SlipId::parse_strict(raw_id).map_err(|_| InvalidReason::BadId)?;
    let created_at = parse_time(raw_created)?;
    let done_at = raw_done_at.map(parse_time).transpose()?;
    let archived_at = raw_archived_at.map(parse_time).transpose()?;

    let decoded = decode(text_part);
    let normalized = normalize_text(&decoded).map_err(map_text_error)?;

    Ok(Some(Record {
        slip: Slip {
            id,
            text: normalized,
            done,
            created_at,
            done_at,
            archived_at,
            locked: raw_locked.unwrap_or(false),
        },
        extras,
    }))
}

fn map_text_error(err: DomainError) -> InvalidReason {
    match err {
        DomainError::EmptyText => InvalidReason::EmptyText,
        DomainError::TextTooLong { .. } => InvalidReason::TextTooLong,
        // 单行内不可能出现换行（换行是行分隔符）；真出现说明 kv 或转义被破坏。
        _ => InvalidReason::BadKeyValue,
    }
}

fn parse_time(raw: &str) -> Result<OffsetDateTime, InvalidReason> {
    let value = OffsetDateTime::parse(raw, &Rfc3339).map_err(|_| InvalidReason::BadTime)?;
    validate_time(value).map_err(|_| InvalidReason::BadTime)?;
    Ok(value)
}

fn valid_key(key: &str) -> bool {
    if key.is_empty() || key.len() > 32 {
        return false;
    }
    let mut chars = key.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

fn valid_value(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|c| !matches!(c, ';' | '=' | '<' | '>') && !c.is_whitespace() && !c.is_control())
}

fn find_last(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    (0..=hay.len() - needle.len())
        .rev()
        .find(|&i| &hay[i..i + needle.len()] == needle)
}

fn encode(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;")
}

fn decode(text: &str) -> String {
    text.replace("&lt;", "<").replace("&amp;", "&")
}

// ---------------------------------------------------------------- 渲染

/// 由 `Document` 渲染整份文件字节。
///
/// **逐行拼接 `Line::raw`，不重新序列化任何行。**
pub fn render(document: &Document) -> Vec<u8> {
    let mut out = Vec::new();
    if document.bom {
        out.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
    }
    for line in &document.lines {
        out.extend_from_slice(&line.raw);
    }
    out
}

// ---------------------------------------------------------------- 应用

/// 把一批 mutation 应用到文档副本上。返回新文档与"是否真的改变"。
pub fn apply(document: &Document, mutations: &[Mutation]) -> Result<(Document, bool), ApplyError> {
    let mut doc = document.clone();
    let mut changed = false;

    // 先 Patch：不改变行数，行索引稳定。
    for mutation in mutations {
        let Mutation::Patch { id, patch } = mutation else {
            continue;
        };
        let record_index = doc
            .records
            .iter()
            .position(|r| r.slip.id == *id)
            .ok_or(ApplyError::SlipNotFound { id: *id })?;

        let (next_slip, transition) =
            apply_patch(&doc.records[record_index].slip, patch).map_err(ApplyError::Domain)?;
        if transition == Transition::Noop {
            continue;
        }

        let line_index = doc
            .lines
            .iter()
            .position(|l| {
                l.kind
                    == LineKind::Record {
                        index: record_index,
                    }
            })
            .ok_or(ApplyError::SlipNotFound { id: *id })?;

        let terminator = doc.lines[line_index].terminator;
        let rewritten = rewrite_line(&doc.lines[line_index].raw, terminator, &next_slip);
        doc.lines[line_index].raw = rewritten;
        doc.records[record_index].slip = next_slip;
        changed = true;
    }

    // 再 Append：只追加到末尾。
    let appends: Vec<&Slip> = mutations
        .iter()
        .filter_map(|m| match m {
            Mutation::Append(slip) => Some(slip),
            Mutation::Patch { .. } => None,
        })
        .collect();

    if !appends.is_empty() {
        for (i, slip) in appends.iter().enumerate() {
            let clashes_existing = doc.records.iter().any(|r| r.slip.id == slip.id);
            let clashes_batch = appends[..i].iter().any(|other| other.id == slip.id);
            if clashes_existing || clashes_batch {
                return Err(ApplyError::DuplicateId { id: slip.id });
            }
        }

        let separator = doc.preferred_terminator.unwrap_or(Terminator::Lf);
        doc.preferred_terminator = Some(separator);

        // 原末行没有终止符时先补一个分隔符，否则新记录会粘在上一行末尾。
        if let Some(last) = doc.lines.last_mut()
            && last.terminator == Terminator::None
        {
            last.raw.extend_from_slice(separator.as_bytes());
            last.terminator = separator;
        }

        let total = appends.len();
        for (i, slip) in appends.iter().enumerate() {
            let terminator = if i + 1 == total && !doc.ends_with_terminator {
                Terminator::None
            } else {
                separator
            };
            let index = doc.records.len();
            doc.lines.push(Line {
                raw: build_line(slip, &[], terminator),
                terminator,
                kind: LineKind::Record { index },
            });
            doc.records.push(Record {
                slip: (*slip).clone(),
                extras: Vec::new(),
            });
        }
        doc.ends_with_terminator = doc
            .lines
            .last()
            .is_some_and(|l| l.terminator != Terminator::None);
        changed = true;
    }

    if changed {
        // §4.1：Noop 不重写 BOM；真正提交时统一输出无 BOM。
        doc.bom = false;
    }

    Ok((doc, changed))
}

/// 构造一条记录行的完整字节（含终止符）。
///
/// 已知字段顺序固定为 `id`、`created`、`done_at`、`archived_at`；
/// 未知片段按原顺序接在后面。checkbox 统一写小写 `x`。
fn build_line(slip: &Slip, extras: &[ExtraField], terminator: Terminator) -> Vec<u8> {
    let mut text = String::new();
    text.push_str(if slip.done { "- [x] " } else { "- [ ] " });
    text.push_str(&encode(&slip.text));
    text.push_str(MARKER_PREFIX);
    text.push_str(HEADER);
    text.push_str(";id=");
    text.push_str(&slip.id.to_canonical_string());
    text.push_str(";created=");
    text.push_str(&format_time(slip.created_at));
    if let Some(done_at) = slip.done_at {
        text.push_str(";done_at=");
        text.push_str(&format_time(done_at));
    }
    if let Some(archived_at) = slip.archived_at {
        text.push_str(";archived_at=");
        text.push_str(&format_time(archived_at));
    }
    if slip.locked {
        // 未锁定时不写这个键：缺省即未锁，老版本读到也不会当成未知字段。
        text.push_str(";locked=1");
    }
    for extra in extras {
        text.push(';');
        text.push_str(&extra.raw);
    }
    text.push_str(MARKER_SUFFIX);

    let mut raw = text.into_bytes();
    raw.extend_from_slice(terminator.as_bytes());
    raw
}

/// 重写一条已存在的记录行：正文与 checkbox 规范化，
/// 已知字段**原位**替换，未知片段逐字保留在原来的相对位置。
fn rewrite_line(raw: &[u8], terminator: Terminator, slip: &Slip) -> Vec<u8> {
    let content = &raw[..raw.len().saturating_sub(terminator.as_bytes().len())];
    let Ok(text) = std::str::from_utf8(content) else {
        return build_line(slip, &[], terminator);
    };
    let Some(pos) = find_last(content, MARKER_NEEDLE) else {
        return build_line(slip, &[], terminator);
    };
    let marker = &text[pos..];
    let Some(inner) = marker
        .strip_prefix(MARKER_PREFIX)
        .and_then(|rest| rest.strip_suffix(MARKER_SUFFIX))
    else {
        return build_line(slip, &[], terminator);
    };
    let kv_part = inner
        .strip_prefix(HEADER)
        .and_then(|rest| rest.strip_prefix(';'))
        .unwrap_or("");

    let mut out = String::new();
    out.push_str(if slip.done { "- [x] " } else { "- [ ] " });
    out.push_str(&encode(&slip.text));
    out.push_str(MARKER_PREFIX);
    out.push_str(HEADER);

    let mut wrote_done_at = false;
    let mut wrote_archived_at = false;
    let mut wrote_locked = false;
    for segment in kv_part.split(';').filter(|segment| !segment.is_empty()) {
        let key = segment
            .split_once('=')
            .map(|(key, _)| key)
            .unwrap_or(segment);
        match key {
            "id" => {
                out.push_str(";id=");
                out.push_str(&slip.id.to_canonical_string());
            }
            "created" => {
                out.push_str(";created=");
                out.push_str(&format_time(slip.created_at));
            }
            "done_at" => {
                wrote_done_at = true;
                if let Some(done_at) = slip.done_at {
                    out.push_str(";done_at=");
                    out.push_str(&format_time(done_at));
                }
            }
            "archived_at" => {
                wrote_archived_at = true;
                if let Some(archived_at) = slip.archived_at {
                    out.push_str(";archived_at=");
                    out.push_str(&format_time(archived_at));
                }
            }
            "locked" => {
                wrote_locked = true;
                if slip.locked {
                    out.push_str(";locked=1");
                }
            }
            _ => {
                out.push(';');
                out.push_str(segment);
            }
        }
    }
    if let Some(done_at) = slip.done_at.filter(|_| !wrote_done_at) {
        out.push_str(";done_at=");
        out.push_str(&format_time(done_at));
    }
    if let Some(archived_at) = slip.archived_at.filter(|_| !wrote_archived_at) {
        out.push_str(";archived_at=");
        out.push_str(&format_time(archived_at));
    }
    if slip.locked && !wrote_locked {
        out.push_str(";locked=1");
    }
    out.push_str(MARKER_SUFFIX);

    let mut raw = out.into_bytes();
    raw.extend_from_slice(terminator.as_bytes());
    raw
}

fn format_time(value: OffsetDateTime) -> String {
    value
        .format(&Rfc3339)
        .unwrap_or_else(|_| String::from("1970-01-01T00:00:00Z"))
}

// ---------------------------------------------------------------- 回读闭包

/// 候选字节 → 重新解析 → 对比记录 + 对比未修改行字节。
pub fn verify_roundtrip(
    original: &Document,
    candidate: &[u8],
    expected: &[Slip],
) -> Result<(), RoundtripError> {
    // 先查重复 ID：这是数据本身的缺陷，应该先于字节比较报告。
    let mut seen = std::collections::HashSet::new();
    for slip in expected {
        if !seen.insert(slip.id) {
            return Err(RoundtripError::DuplicateId);
        }
    }

    let reparsed = parse(candidate);
    if !reparsed.problems.is_empty() {
        return Err(RoundtripError::ReparseFailed);
    }

    let got: Vec<&Slip> = reparsed.document.records.iter().map(|r| &r.slip).collect();
    if got.len() != expected.len() {
        return Err(RoundtripError::RecordMismatch {
            index: got.len().min(expected.len()),
        });
    }
    for (index, (actual, want)) in got.iter().zip(expected.iter()).enumerate() {
        if **actual != *want {
            return Err(RoundtripError::RecordMismatch { index });
        }
    }

    // 未修改行必须逐字节保留（含终止符）。唯一例外：向没有末尾换行的文件追加时，
    // 原末行会补一个分隔符；除此之外行数也不得变化。
    let appended = expected.len().saturating_sub(original.records.len());
    let expected_lines = original.lines.len() + appended;
    if reparsed.document.lines.len() != expected_lines {
        return Err(RoundtripError::UnmodifiedLineChanged {
            line: expected_lines.min(reparsed.document.lines.len()) + 1,
        });
    }

    for (index, line) in original.lines.iter().enumerate() {
        let after = &reparsed.document.lines[index];
        let changed_record = match line.kind {
            LineKind::Record {
                index: record_index,
            } => expected.get(record_index) != original.records.get(record_index).map(|r| &r.slip),
            _ => false,
        };
        if changed_record {
            continue;
        }

        let separator_added = index + 1 == original.lines.len()
            && appended > 0
            && line.terminator == Terminator::None
            && strip_terminator(line) == strip_terminator(after)
            && after.terminator != Terminator::None;
        if line.raw != after.raw && !separator_added {
            return Err(RoundtripError::UnmodifiedLineChanged { line: index + 1 });
        }
    }

    Ok(())
}

fn strip_terminator(line: &Line) -> &[u8] {
    let len = line.terminator.as_bytes().len();
    &line.raw[..line.raw.len().saturating_sub(len)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use slip_domain::Patch;
    use time::macros::datetime;

    const ID: &str = "019a9f3c-2b70-7d84-a1c6-8e3f5b0d2741";
    const ID2: &str = "019a9f3c-2b70-7d84-a1c6-8e3f5b0d2742";
    const CREATED: &str = "2026-11-10T11:49:14+08:00";

    fn record_line(text: &str) -> String {
        format!("- [ ] {text} <!-- slip:v1;id={ID};created={CREATED} -->\n")
    }

    fn parse_ok(bytes: &[u8]) -> Document {
        let parsed = parse(bytes);
        assert!(
            parsed.problems.is_empty(),
            "unexpected problems: {:?}",
            parsed.problems
        );
        parsed.document
    }

    fn sample_slip(text: &str) -> Slip {
        slip_with_id(ID, text)
    }

    fn slip_with_id(id: &str, text: &str) -> Slip {
        Slip {
            id: SlipId::parse_strict(id).unwrap(),
            text: text.to_owned(),
            done: false,
            created_at: datetime!(2026-11-10 11:49:14 +08:00),
            done_at: None,
            archived_at: None,
            locked: false,
        }
    }

    const ID3: &str = "019a9f3c-2b70-7d84-a1c6-8e3f5b0d2743";

    // ---------------- 基础解析 ----------------

    #[test]
    fn parses_valid_record() {
        let doc = parse_ok(record_line("买发票").as_bytes());
        assert_eq!(doc.records.len(), 1);
        assert_eq!(doc.records[0].slip.text, "买发票");
        assert!(!doc.records[0].slip.done);
        assert_eq!(doc.lines.len(), 1);
    }

    #[test]
    fn uppercase_x_means_done() {
        let raw = format!("- [X] 已完成 <!-- slip:v1;id={ID};created={CREATED} -->\n");
        let doc = parse_ok(raw.as_bytes());
        assert!(doc.records[0].slip.done);
    }

    #[test]
    fn non_candidate_checkbox_is_other() {
        let doc = parse_ok(b"- [ ] plain todo without marker\n");
        assert!(doc.records.is_empty());
        assert_eq!(doc.lines[0].kind, LineKind::Other);
    }

    #[test]
    fn empty_text_record_is_invalid_not_panic() {
        // marker needle 自带前导空格；正文为空时它落在 checkbox 后的空格上。
        let raw = format!("- [ ] <!-- slip:v1;id={ID};created={CREATED} -->\n");
        let parsed = parse(raw.as_bytes());
        assert!(parsed.document.records.is_empty());
        assert_eq!(parsed.problems.len(), 1);
        assert_eq!(parsed.problems[0].reason, InvalidReason::EmptyText);
    }

    #[test]
    fn heading_is_other() {
        let doc = parse_ok(b"# Inbox\n");
        assert_eq!(doc.lines[0].kind, LineKind::Other);
    }

    // ---------------- 文法 ----------------

    #[test]
    fn semicolon_in_unknown_value_is_bad_key_value() {
        // `extra=a;b` 切出 `b`，没有 `=` → 唯一结果 BadKeyValue。
        let raw = format!("- [ ] t <!-- slip:v1;id={ID};created={CREATED};extra=a;b -->\n");
        let parsed = parse(raw.as_bytes());
        assert_eq!(parsed.problems.len(), 1);
        assert_eq!(parsed.problems[0].reason, InvalidReason::BadKeyValue);
    }

    #[test]
    fn two_unknown_kvs_are_accepted() {
        let raw = format!("- [ ] t <!-- slip:v1;id={ID};created={CREATED};a=1;b=2 -->\n");
        let doc = parse_ok(raw.as_bytes());
        assert_eq!(doc.records[0].extras.len(), 2);
        assert_eq!(doc.records[0].extras[0].raw, "a=1");
        assert_eq!(doc.records[0].extras[1].raw, "b=2");
    }

    #[test]
    fn duplicate_known_key_rejected() {
        let raw = format!("- [ ] t <!-- slip:v1;id={ID};created={CREATED};created={CREATED} -->\n");
        let parsed = parse(raw.as_bytes());
        assert_eq!(parsed.problems[0].reason, InvalidReason::DuplicateKey);
    }

    #[test]
    fn unsupported_version_rejected() {
        let raw = format!("- [ ] t <!-- slip:v2;id={ID};created={CREATED} -->\n");
        let parsed = parse(raw.as_bytes());
        assert_eq!(parsed.problems[0].reason, InvalidReason::UnsupportedVersion);
    }

    #[test]
    fn missing_id_rejected() {
        let raw = format!("- [ ] t <!-- slip:v1;created={CREATED} -->\n");
        let parsed = parse(raw.as_bytes());
        assert_eq!(parsed.problems[0].reason, InvalidReason::MissingField);
    }

    #[test]
    fn bad_id_rejected() {
        let raw = format!("- [ ] t <!-- slip:v1;id=not-a-uuid;created={CREATED} -->\n");
        let parsed = parse(raw.as_bytes());
        assert_eq!(parsed.problems[0].reason, InvalidReason::BadId);
    }

    #[test]
    fn uppercase_uuid_rejected() {
        let upper = ID.to_uppercase();
        let raw = format!("- [ ] t <!-- slip:v1;id={upper};created={CREATED} -->\n");
        let parsed = parse(raw.as_bytes());
        assert_eq!(parsed.problems[0].reason, InvalidReason::BadId);
    }

    #[test]
    fn bad_time_rejected() {
        let raw = format!("- [ ] t <!-- slip:v1;id={ID};created=yesterday -->\n");
        let parsed = parse(raw.as_bytes());
        assert_eq!(parsed.problems[0].reason, InvalidReason::BadTime);
    }

    #[test]
    fn empty_text_rejected() {
        let raw = format!("- [ ]  <!-- slip:v1;id={ID};created={CREATED} -->\n");
        let parsed = parse(raw.as_bytes());
        assert_eq!(parsed.problems[0].reason, InvalidReason::EmptyText);
    }

    #[test]
    fn non_utf8_candidate_rejected() {
        let mut raw = b"- [ ] ".to_vec();
        raw.extend_from_slice(&[0xFF, 0xFE]);
        raw.extend_from_slice(b" <!-- slip:v1;id=");
        raw.extend_from_slice(ID.as_bytes());
        raw.extend_from_slice(b";created=");
        raw.extend_from_slice(CREATED.as_bytes());
        raw.extend_from_slice(b" -->\n");
        let parsed = parse(&raw);
        assert_eq!(parsed.problems[0].reason, InvalidReason::NonUtf8);
    }

    // ---------------- 转义往返 ----------------

    #[test]
    fn encodes_ampersand_and_angle() {
        assert_eq!(encode("a&b<c"), "a&amp;b&lt;c");
        assert_eq!(decode("a&amp;b&lt;c"), "a&b<c");
    }

    #[test]
    fn literal_entity_round_trips() {
        let original = "&lt;";
        assert_eq!(decode(&encode(original)), original);
    }

    #[test]
    fn special_text_survives_round_trip() {
        for text in [
            "a&b",
            "a<b",
            "a&amp;b",
            "C:\\temp",
            "买发票",
            "😀",
            "<!-- not a marker",
        ] {
            let mut slip = sample_slip(text);
            slip.text = normalize_text(text).unwrap();
            let doc = Document {
                bom: false,
                lines: Vec::new(),
                records: Vec::new(),
                preferred_terminator: Some(Terminator::Lf),
                ends_with_terminator: false,
            };
            let (next, _) = apply(&doc, &[Mutation::Append(slip.clone())]).unwrap();
            let bytes = render(&next);
            let reparsed = parse_ok(&bytes);
            assert_eq!(reparsed.records[0].slip.text, slip.text, "text={text}");
        }
    }

    // ---------------- 换行 ----------------

    #[test]
    fn splits_crlf_lf_cr() {
        let doc = parse_ok(b"a\r\nb\nc\rd");
        assert_eq!(doc.lines.len(), 4);
        assert_eq!(doc.lines[0].terminator, Terminator::CrLf);
        assert_eq!(doc.lines[1].terminator, Terminator::Lf);
        assert_eq!(doc.lines[2].terminator, Terminator::Cr);
        assert_eq!(doc.lines[3].terminator, Terminator::None);
        assert!(!doc.ends_with_terminator);
    }

    #[test]
    fn no_trailing_newline_flag() {
        let doc = parse_ok(b"a");
        assert!(!doc.ends_with_terminator);
        let doc = parse_ok(b"a\n");
        assert!(doc.ends_with_terminator);
    }

    #[test]
    fn empty_file_parses() {
        let doc = parse_ok(b"");
        assert!(doc.lines.is_empty());
        assert!(doc.records.is_empty());
        assert_eq!(doc.preferred_terminator, None);
        assert!(!doc.ends_with_terminator);
    }

    #[test]
    fn bom_is_stripped_from_view_and_restored() {
        let mut raw = vec![0xEF, 0xBB, 0xBF];
        raw.extend_from_slice(b"# Inbox\n");
        let doc = parse_ok(&raw);
        assert!(doc.bom);
        assert_eq!(doc.lines[0].raw, b"# Inbox\n");
        assert_eq!(render(&doc), raw);
    }

    #[test]
    fn preferred_terminator_is_first_occurrence() {
        let doc = parse_ok(b"a\r\nb\nc\r\n");
        assert_eq!(doc.preferred_terminator, Some(Terminator::CrLf));
    }

    // ---------------- 渲染保持原字节 ----------------

    #[test]
    fn render_reproduces_original_bytes() {
        let raw = record_line("买发票")
            .into_bytes()
            .into_iter()
            .chain(b"# heading\n".iter().copied())
            .collect::<Vec<u8>>();
        let doc = parse_ok(&raw);
        assert_eq!(render(&doc), raw);
    }

    #[test]
    fn preserves_unknown_kv_order_and_raw() {
        let raw = format!("- [ ] t <!-- slip:v1;zz=9;id={ID};aa=1;created={CREATED} -->\n");
        let doc = parse_ok(raw.as_bytes());
        assert_eq!(doc.records[0].extras[0].raw, "zz=9");
        assert_eq!(doc.records[0].extras[1].raw, "aa=1");
        // 未修改 → 字节完全不变
        assert_eq!(render(&doc), raw.as_bytes());
    }

    // ---------------- 追加 ----------------

    #[test]
    fn appends_three_to_empty_without_trailing_newline() {
        let doc = parse_ok(b"");
        let slips = [
            slip_with_id(ID, "a"),
            slip_with_id(ID2, "b"),
            slip_with_id(ID3, "c"),
        ];
        let (next, changed) = apply(
            &doc,
            &[
                Mutation::Append(slips[0].clone()),
                Mutation::Append(slips[1].clone()),
                Mutation::Append(slips[2].clone()),
            ],
        )
        .unwrap();
        assert!(changed);
        let bytes = render(&next);
        assert_eq!(bytes.iter().filter(|b| **b == b'\n').count(), 2);
        assert!(!bytes.ends_with(b"\n"));
        assert_eq!(parse_ok(&bytes).records.len(), 3);
    }

    #[test]
    fn append_adds_separator_when_last_line_lacks_terminator() {
        let doc = parse_ok(b"# heading");
        let (next, _) = apply(&doc, &[Mutation::Append(sample_slip("a"))]).unwrap();
        let bytes = render(&next);
        assert_eq!(bytes, b"# heading\n- [ ] a <!-- slip:v1;id=019a9f3c-2b70-7d84-a1c6-8e3f5b0d2741;created=2026-11-10T11:49:14+08:00 -->".to_vec());
    }

    #[test]
    fn append_inherits_eof_property() {
        let doc = parse_ok(b"# heading\n");
        let (next, _) = apply(&doc, &[Mutation::Append(sample_slip("a"))]).unwrap();
        assert!(render(&next).ends_with(b"\n"));
    }

    #[test]
    fn append_duplicate_id_rejected() {
        let doc = parse_ok(record_line("a").as_bytes());
        let err = apply(&doc, &[Mutation::Append(sample_slip("b"))]).unwrap_err();
        assert!(matches!(err, ApplyError::DuplicateId { .. }));
    }

    // ---------------- Patch ----------------

    #[test]
    fn patch_set_text_rewrites_line() {
        let doc = parse_ok(record_line("旧").as_bytes());
        let id = SlipId::parse_strict(ID).unwrap();
        let (next, changed) = apply(
            &doc,
            &[Mutation::Patch {
                id,
                patch: Patch::SetText("新".to_owned()),
            }],
        )
        .unwrap();
        assert!(changed);
        assert_eq!(next.records[0].slip.text, "新");
        assert_eq!(parse_ok(&render(&next)).records[0].slip.text, "新");
    }

    #[test]
    fn patch_noop_keeps_raw_bytes() {
        let raw = format!("- [ ] t <!-- slip:v1;zz=9;id={ID};created={CREATED} -->\n");
        let doc = parse_ok(raw.as_bytes());
        let id = SlipId::parse_strict(ID).unwrap();
        let (next, changed) = apply(
            &doc,
            &[Mutation::Patch {
                id,
                patch: Patch::SetText("t".to_owned()),
            }],
        )
        .unwrap();
        assert!(!changed);
        assert_eq!(next.lines[0].raw, raw.as_bytes());
    }

    #[test]
    fn patch_unknown_id_errors() {
        let doc = parse_ok(record_line("a").as_bytes());
        let id = SlipId::parse_strict(ID2).unwrap();
        let err = apply(
            &doc,
            &[Mutation::Patch {
                id,
                patch: Patch::SetText("x".to_owned()),
            }],
        )
        .unwrap_err();
        assert!(matches!(err, ApplyError::SlipNotFound { .. }));
    }

    #[test]
    fn patch_preserves_extras_on_rewrite() {
        let raw = format!("- [ ] t <!-- slip:v1;id={ID};zz=9;created={CREATED} -->\n");
        let doc = parse_ok(raw.as_bytes());
        let id = SlipId::parse_strict(ID).unwrap();
        let (next, _) = apply(
            &doc,
            &[Mutation::Patch {
                id,
                patch: Patch::SetText("新".to_owned()),
            }],
        )
        .unwrap();
        let bytes = render(&next);
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.contains(";zz=9"), "extras lost: {text}");
    }

    #[test]
    fn patch_keeps_unknown_kv_in_original_positions() {
        let raw = format!("- [ ] 旧 <!-- slip:v1;zz=9;id={ID};created={CREATED};aa=1 -->\n");
        let doc = parse_ok(raw.as_bytes());
        let id = SlipId::parse_strict(ID).unwrap();
        let (next, _) = apply(
            &doc,
            &[Mutation::Patch {
                id,
                patch: Patch::SetText("新".to_owned()),
            }],
        )
        .unwrap();
        let text = String::from_utf8(render(&next)).unwrap();
        assert!(
            text.contains(&format!("slip:v1;zz=9;id={ID};created={CREATED};aa=1 -->")),
            "known fields must be replaced in place: {text}"
        );
    }

    #[test]
    fn commit_strips_bom_but_noop_does_not_change_it() {
        let mut raw = vec![0xEF, 0xBB, 0xBF];
        raw.extend_from_slice(record_line("t").as_bytes());
        let doc = parse_ok(&raw);
        let id = SlipId::parse_strict(ID).unwrap();

        let (same, changed) = apply(
            &doc,
            &[Mutation::Patch {
                id,
                patch: Patch::SetText("t".to_owned()),
            }],
        )
        .unwrap();
        assert!(!changed);
        assert_eq!(render(&same), raw);

        let (next, changed) = apply(
            &doc,
            &[Mutation::Patch {
                id,
                patch: Patch::SetText("新".to_owned()),
            }],
        )
        .unwrap();
        assert!(changed);
        assert!(!next.bom);
        assert!(!render(&next).starts_with(&[0xEF, 0xBB, 0xBF]));
    }

    #[test]
    fn completing_writes_done_at() {
        let doc = parse_ok(record_line("a").as_bytes());
        let id = SlipId::parse_strict(ID).unwrap();
        let (next, _) = apply(
            &doc,
            &[Mutation::Patch {
                id,
                patch: Patch::SetDone {
                    done: true,
                    at: datetime!(2026-11-10 12:00:00 +08:00),
                },
            }],
        )
        .unwrap();
        let text = String::from_utf8(render(&next)).unwrap();
        assert!(text.starts_with("- [x] "));
        assert!(text.contains(";done_at=2026-11-10T12:00:00+08:00"));
        assert!(parse_ok(text.as_bytes()).records[0].slip.done);
    }

    // ---------------- 回读闭包 ----------------

    #[test]
    fn roundtrip_accepts_faithful_candidate() {
        let doc = parse_ok(record_line("a").as_bytes());
        let candidate = render(&doc);
        let expected = doc
            .records
            .iter()
            .map(|r| r.slip.clone())
            .collect::<Vec<_>>();
        verify_roundtrip(&doc, &candidate, &expected).unwrap();
    }

    #[test]
    fn roundtrip_detects_tampered_unmodified_line() {
        let raw = b"# heading\n- [ ] a <!-- slip:v1;id=019a9f3c-2b70-7d84-a1c6-8e3f5b0d2741;created=2026-11-10T11:49:14+08:00 -->\n".to_vec();
        let doc = parse_ok(&raw);
        let mut candidate = raw.clone();
        candidate[2] = b'H'; // "# heading" → "# Heading"
        let expected = doc
            .records
            .iter()
            .map(|r| r.slip.clone())
            .collect::<Vec<_>>();
        assert_eq!(
            verify_roundtrip(&doc, &candidate, &expected),
            Err(RoundtripError::UnmodifiedLineChanged { line: 1 })
        );
    }

    #[test]
    fn roundtrip_detects_record_mismatch() {
        let doc = parse_ok(record_line("a").as_bytes());
        let candidate = render(&doc);
        let expected = vec![sample_slip("different")];
        assert!(matches!(
            verify_roundtrip(&doc, &candidate, &expected),
            Err(RoundtripError::RecordMismatch { .. })
        ));
    }

    #[test]
    fn roundtrip_detects_duplicate_ids() {
        let doc = parse_ok(record_line("a").as_bytes());
        let candidate = render(&doc);
        let slip = sample_slip("a");
        let expected = vec![slip.clone(), slip];
        assert_eq!(
            verify_roundtrip(&doc, &candidate, &expected),
            Err(RoundtripError::DuplicateId)
        );
    }

    #[test]
    fn locked_round_trips_through_the_marker() {
        let raw = format!("- [ ] 定稿 <!-- slip:v1;id={ID};created={CREATED};locked=1 -->\n");
        let doc = parse_ok(raw.as_bytes());
        assert!(doc.records[0].slip.locked);
        // 没有改动时字节必须原样：锁定不该引起任何重写。
        assert_eq!(render(&doc), raw.as_bytes());
    }

    #[test]
    fn without_the_key_is_unlocked() {
        let doc = parse_ok(record_line("普通").as_bytes());
        assert!(!doc.records[0].slip.locked);
    }

    #[test]
    fn lock_toggle_writes_in_place_and_keeps_unknown_kv() {
        let raw = format!("- [ ] 旧 <!-- slip:v1;zz=9;id={ID};created={CREATED} -->\n");
        let doc = parse_ok(raw.as_bytes());
        let id = SlipId::parse_strict(ID).unwrap();

        let (next, _) = apply(
            &doc,
            &[Mutation::Patch {
                id,
                patch: Patch::SetLocked(true),
            }],
        )
        .unwrap();
        let text = String::from_utf8(render(&next)).unwrap();
        assert!(text.contains("zz=9;id="), "unknown kv moved: {text}");
        assert!(text.contains(";locked=1"), "lock not persisted: {text}");
    }

    #[test]
    fn bad_locked_value_is_reported_not_guessed() {
        let raw = format!("- [ ] t <!-- slip:v1;id={ID};created={CREATED};locked=yes -->\n");
        let parsed = parse(raw.as_bytes());
        assert!(parsed.document.records.is_empty());
        assert_eq!(parsed.problems.len(), 1, "{:?}", parsed.problems);
    }
}
