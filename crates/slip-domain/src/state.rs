//! 状态转换与领域校验（v11 §1.3、§1.4）。
//!
//! 两条原则，来自前几轮的教训：
//!
//! 1. **应用行为 ≠ 文件约束。** 转换表规定"应用怎么写"，不规定"文件必须长什么样"。
//! 2. **合法性只认可解析、可往返。** 不检查时间先后，不检查跨字段蕴含
//!    （`done ⇒ done_at` 之类）——否则用户手动把 `[ ]` 改成 `[x]` 就会让整库只读，
//!    而且系统时钟回拨会让保存失败。

use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::error::DomainError;
use crate::model::{Patch, Slip, Transition};
use crate::text::{MAX_TEXT_CODEPOINTS, codepoint_len, normalize_text};

/// 校验一个时间能否用**文件实际使用的格式**往返。
///
/// 不能只检查"它是 `OffsetDateTime`"：类型能装下的值，RFC3339 未必写得出来
/// （年份越界、偏移含秒等）。判断标准是 format → parse 后仍是同一个时刻。
pub fn validate_time(value: OffsetDateTime) -> Result<(), DomainError> {
    let text = value
        .format(&Rfc3339)
        .map_err(|_| DomainError::TimestampNotSerializable)?;
    let parsed = OffsetDateTime::parse(&text, &Rfc3339)
        .map_err(|_| DomainError::TimestampNotSerializable)?;
    if parsed != value {
        return Err(DomainError::TimestampNotSerializable);
    }
    Ok(())
}

/// 校验一张纸条是否符合文件格式能表达的范围。
///
/// **只检查这三类**（v11 §1.4）：
/// - 文本：非空、长度合法、无 CR/LF、且已是 `normalize_text` 的规范形式；
/// - ID：非 nil、版本 7、小写连字符规范形式；
/// - 时间：可用 RFC3339 往返。
///
/// 明确**不检查**：`created_at` 与 `done_at`/`archived_at` 的先后；
/// `done` 与时间字段之间的蕴含关系。
pub fn validate_slip(slip: &Slip) -> Result<(), DomainError> {
    // 文本
    if slip.text.contains('\r') || slip.text.contains('\n') {
        return Err(DomainError::TextContainsNewline);
    }
    if slip.text.trim().is_empty() {
        return Err(DomainError::EmptyText);
    }
    let len = codepoint_len(&slip.text);
    if len > MAX_TEXT_CODEPOINTS {
        return Err(DomainError::TextTooLong {
            actual: len,
            max: MAX_TEXT_CODEPOINTS,
        });
    }
    if slip.text.trim() != slip.text {
        return Err(DomainError::TextNotNormalized);
    }

    // ID
    slip.id.validate()?;

    // 时间
    validate_time(slip.created_at)?;
    if let Some(at) = slip.done_at {
        validate_time(at)?;
    }
    if let Some(at) = slip.archived_at {
        validate_time(at)?;
    }

    Ok(())
}

/// 应用一次 `Patch`，返回新值与是否真正改变。
///
/// 转换规则是**应用行为**，用于决定"应用写什么"。
/// 文件里出现不符合转换表的组合仍然可读（例如未完成但残留 `archived_at`）。
pub fn apply_patch(slip: &Slip, patch: &Patch) -> Result<(Slip, Transition), DomainError> {
    match patch {
        Patch::SetText(raw) => {
            let text = normalize_text(raw)?;
            if text == slip.text {
                return Ok((slip.clone(), Transition::Noop));
            }
            let mut next = slip.clone();
            next.text = text;
            Ok((next, Transition::Changed))
        }

        Patch::SetDone { done: true, at } => {
            // 已经完成：不补写缺失的 done_at，也不重写原时间。
            if slip.done {
                return Ok((slip.clone(), Transition::Noop));
            }
            let mut next = slip.clone();
            next.done = true;
            next.done_at = Some(*at);
            // 清掉外部残留的归档时间，避免"刚点完成就被判定为已归档而消失"。
            next.archived_at = None;
            Ok((next, Transition::Changed))
        }

        Patch::SetDone { done: false, .. } => {
            let already = !slip.done && slip.done_at.is_none() && slip.archived_at.is_none();
            if already {
                return Ok((slip.clone(), Transition::Noop));
            }
            let mut next = slip.clone();
            next.done = false;
            next.done_at = None;
            // 取消完成同时取消归档，避免产生永恒隐藏的条目。
            next.archived_at = None;
            Ok((next, Transition::Changed))
        }

        Patch::Archive { at } => {
            if !slip.done {
                return Err(DomainError::ArchiveRequiresDone);
            }
            if slip.archived_at.is_some() {
                // 已有效归档：保留原归档时间。
                return Ok((slip.clone(), Transition::Noop));
            }
            let mut next = slip.clone();
            next.archived_at = Some(*at);
            Ok((next, Transition::Changed))
        }
    }
}

#[cfg(test)]
mod tests {
    use time::macros::datetime;

    use super::*;
    use crate::id::SlipId;

    fn at_noon() -> OffsetDateTime {
        datetime!(2026-11-10 12:00:00 +08:00)
    }

    fn at_earlier() -> OffsetDateTime {
        // 比创建时间早 1 分钟：模拟时钟回拨 / 用户改系统时间。
        datetime!(2026-11-10 11:59:00 +08:00)
    }

    fn base_slip() -> Slip {
        Slip {
            id: SlipId::new_v7(),
            text: "买发票".to_owned(),
            done: false,
            created_at: at_noon(),
            done_at: None,
            archived_at: None,
        }
    }

    // ---------- 状态转换 ----------

    #[test]
    fn set_done_true_records_time() {
        let slip = base_slip();
        let (next, t) = apply_patch(
            &slip,
            &Patch::SetDone {
                done: true,
                at: at_noon(),
            },
        )
        .unwrap();
        assert_eq!(t, Transition::Changed);
        assert!(next.done);
        assert_eq!(next.done_at, Some(at_noon()));
    }

    #[test]
    fn set_done_true_clears_residual_archive_time() {
        // 外部手工取消勾选后残留 archived_at；重新完成时不能让它立刻又隐藏。
        let mut slip = base_slip();
        slip.done = true;
        slip.done_at = Some(at_noon());
        slip.archived_at = Some(at_noon());
        slip.done = false; // 模拟"外部把 [x] 改回 [ ]"但归档时间没删
        let (next, _) = apply_patch(
            &slip,
            &Patch::SetDone {
                done: true,
                at: at_noon(),
            },
        )
        .unwrap();
        assert!(next.done);
        assert_eq!(next.archived_at, None);
    }

    #[test]
    fn set_done_true_on_done_is_noop() {
        let mut slip = base_slip();
        slip.done = true;
        slip.done_at = None; // 手工完成、未记录时间
        let (next, t) = apply_patch(
            &slip,
            &Patch::SetDone {
                done: true,
                at: at_noon(),
            },
        )
        .unwrap();
        assert_eq!(t, Transition::Noop);
        // 不补写缺失的 done_at。
        assert_eq!(next.done_at, None);
    }

    #[test]
    fn set_done_false_clears_both_times() {
        let mut slip = base_slip();
        slip.done = true;
        slip.done_at = Some(at_noon());
        slip.archived_at = Some(at_noon());
        let (next, t) = apply_patch(
            &slip,
            &Patch::SetDone {
                done: false,
                at: at_noon(),
            },
        )
        .unwrap();
        assert_eq!(t, Transition::Changed);
        assert!(!next.done);
        assert_eq!(next.done_at, None);
        assert_eq!(next.archived_at, None);
    }

    #[test]
    fn set_done_false_on_open_slip_is_noop() {
        let slip = base_slip();
        let (_, t) = apply_patch(
            &slip,
            &Patch::SetDone {
                done: false,
                at: at_noon(),
            },
        )
        .unwrap();
        assert_eq!(t, Transition::Noop);
    }

    #[test]
    fn archive_requires_done() {
        let slip = base_slip();
        assert_eq!(
            apply_patch(&slip, &Patch::Archive { at: at_noon() }),
            Err(DomainError::ArchiveRequiresDone)
        );
    }

    #[test]
    fn archive_sets_time_when_done() {
        let mut slip = base_slip();
        slip.done = true;
        slip.done_at = Some(at_noon());
        let (next, t) = apply_patch(&slip, &Patch::Archive { at: at_noon() }).unwrap();
        assert_eq!(t, Transition::Changed);
        assert_eq!(next.archived_at, Some(at_noon()));
    }

    #[test]
    fn archive_twice_is_noop_and_keeps_first_time() {
        let mut slip = base_slip();
        slip.done = true;
        slip.done_at = Some(at_noon());
        slip.archived_at = Some(at_earlier());
        let (next, t) = apply_patch(&slip, &Patch::Archive { at: at_noon() }).unwrap();
        assert_eq!(t, Transition::Noop);
        assert_eq!(next.archived_at, Some(at_earlier()));
    }

    #[test]
    fn set_text_same_value_is_noop() {
        let slip = base_slip();
        let (_, t) = apply_patch(&slip, &Patch::SetText("  买发票  ".to_owned())).unwrap();
        assert_eq!(t, Transition::Noop);
    }

    #[test]
    fn set_text_normalizes_and_marks_changed() {
        let slip = base_slip();
        let (next, t) = apply_patch(&slip, &Patch::SetText("  新内容 ".to_owned())).unwrap();
        assert_eq!(t, Transition::Changed);
        assert_eq!(next.text, "新内容");
    }

    #[test]
    fn set_text_rejects_invalid_input() {
        let slip = base_slip();
        assert_eq!(
            apply_patch(&slip, &Patch::SetText("   ".to_owned())),
            Err(DomainError::EmptyText)
        );
        assert_eq!(
            apply_patch(&slip, &Patch::SetText("a\nb".to_owned())),
            Err(DomainError::TextContainsNewline)
        );
    }

    // ---------- 校验：时间与 ID ----------

    #[test]
    fn accepts_clock_rollback() {
        // 完成时间早于创建时间：合法。时间不是顺序判据（v11 §1.5）。
        let mut slip = base_slip();
        slip.done = true;
        slip.done_at = Some(at_earlier());
        validate_slip(&slip).unwrap();
    }

    #[test]
    fn accepts_done_without_done_at() {
        // 用户手工把 [ ] 改成 [x]：不能在读取时判为损坏。
        let mut slip = base_slip();
        slip.done = true;
        slip.done_at = None;
        validate_slip(&slip).unwrap();
    }

    #[test]
    fn accepts_done_at_without_done() {
        // 用户手工把 [x] 改回 [ ] 但没删 done_at：仍然可读。
        let mut slip = base_slip();
        slip.done = false;
        slip.done_at = Some(at_noon());
        validate_slip(&slip).unwrap();
    }

    #[test]
    fn accepts_archived_at_without_done() {
        let mut slip = base_slip();
        slip.done = false;
        slip.archived_at = Some(at_noon());
        validate_slip(&slip).unwrap();
    }

    #[test]
    fn rejects_timestamp_with_second_precision_offset() {
        // RFC3339 的偏移只有 ±HH:MM，含秒的偏移写不出来。
        let offset = time::UtcOffset::from_hms(8, 0, 30).unwrap();
        let value = slip_with_offset(offset);
        assert_eq!(
            validate_time(value),
            Err(DomainError::TimestampNotSerializable)
        );
    }

    fn slip_with_offset(offset: time::UtcOffset) -> OffsetDateTime {
        let date = time::Date::from_calendar_date(2026, time::Month::November, 10).unwrap();
        let time = time::Time::from_hms(12, 0, 0).unwrap();
        time::PrimitiveDateTime::new(date, time).assume_offset(offset)
    }

    #[test]
    fn rejects_unnormalized_text() {
        let mut slip = base_slip();
        slip.text = " 买发票".to_owned();
        assert_eq!(validate_slip(&slip), Err(DomainError::TextNotNormalized));
    }

    #[test]
    fn rejects_text_over_limit() {
        let mut slip = base_slip();
        slip.text = "a".repeat(MAX_TEXT_CODEPOINTS + 1);
        assert!(matches!(
            validate_slip(&slip),
            Err(DomainError::TextTooLong { .. })
        ));
    }

    #[test]
    fn every_transition_stays_valid() {
        // v11 §1.3 要求：转换结果必须仍能通过校验（否则会写出 reload 拒绝的数据）。
        let slip = base_slip();
        let cases = [
            Patch::SetText("改个说法".to_owned()),
            Patch::SetDone {
                done: true,
                at: at_earlier(),
            },
            Patch::SetDone {
                done: false,
                at: at_noon(),
            },
            Patch::Archive { at: at_noon() },
        ];
        for patch in cases {
            match apply_patch(&slip, &patch) {
                Ok((next, _)) => validate_slip(&next).unwrap(),
                // Archive 在未完成时按规则拒绝，这是预期结果。
                Err(DomainError::ArchiveRequiresDone) => {}
                Err(e) => panic!("unexpected error: {e}"),
            }
        }
    }
}
