//! 归档墓碑的**只读**容量统计（R102A）。
//!
//! 为什么是只读：设计里有一条硬不变量——`inbox.md` 的未修改行必须**逐字节保留**，
//! 且禁止全文重序列化（`AGENTS.md`）。原矩阵设想的"整文件 compact 物理清除墓碑"
//! 恰好同时违反这两条，因此被改成：**先让用户看见能清理多少，再由他逐条决定**。
//!
//! 这个模块只回答"有多少"，不回答"怎么删"——删除是单条事务的事（在 `slip-store` 的
//! 常规写路径上），不能在这里顺手做掉。

use crate::IoKind;
use crate::doc::{LineKind, parse};
use crate::fsops::map_io;
use std::path::Path;

/// 归档墓碑的容量统计。
///
/// `archived_bytes` 统计的是这些记录**在文件里占的原始字节**（含终止符），
/// 而不是纸条正文的长度——用户关心的是"清掉能省多少磁盘"。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ArchivedCapacity {
    /// 有效归档（`done && archived_at.is_some()`）的条数。
    pub archived: usize,
    /// 这些归档记录在文件里占的字节数。
    pub archived_bytes: u64,
    /// 合法记录总数。
    pub total_records: usize,
    /// 整份文件的字节数（含 BOM、空行、注释等一切内容）。
    pub total_bytes: u64,
}

impl ArchivedCapacity {
    /// 归档占比（0.0–1.0）。文件为空时返回 0.0 而不是 NaN。
    #[must_use]
    pub fn archived_ratio(&self) -> f64 {
        if self.total_bytes == 0 {
            return 0.0;
        }
        self.archived_bytes as f64 / self.total_bytes as f64
    }
}

/// 统计一份 `inbox.md` 字节里的归档墓碑容量。
///
/// **只读**：不写盘、不改文档。解析失败的记录不计入 `archived`（它们不是墓碑，
/// 是解析问题，由诊断面板负责报告）。
///
/// 口径与 `presenter` 的 `SlipView::archived` 一致：**有效归档 = 已完成且 `archived_at` 有值**。
/// 未完成但残留 `archived_at` 的条目不隐藏、也不计入（这正是 v11 §1.8 的算法）。
#[must_use]
pub fn archived_capacity(bytes: &[u8]) -> ArchivedCapacity {
    let parsed = parse(bytes);
    let document = &parsed.document;

    let mut capacity = ArchivedCapacity {
        total_records: document.records.len(),
        total_bytes: bytes.len() as u64,
        ..ArchivedCapacity::default()
    };

    for line in &document.lines {
        let LineKind::Record { index } = line.kind else {
            continue;
        };
        let Some(record) = document.records.get(index) else {
            continue;
        };
        let slip = &record.slip;
        if slip.done && slip.archived_at.is_some() {
            capacity.archived += 1;
            capacity.archived_bytes += line.raw.len() as u64;
        }
    }

    capacity
}

/// 从文件读取并统计归档容量。
///
/// 存在的理由：**外壳不得直接读 `inbox.md`**（读也是 store 的职责）。
/// 外壳只调用这个入口拿统计结果，自己不开文件句柄。
pub fn archived_capacity_at(path: &Path) -> Result<ArchivedCapacity, IoKind> {
    let bytes = std::fs::read(path).map_err(map_io)?;
    Ok(archived_capacity(&bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marker(id: u8, done: bool, archived: bool) -> String {
        // 一份最小的合法记录。用一个确定的 UUID 形状，避免依赖随机生成。
        let id = format!("019a9f3c-2b70-7d84-a1c6-8e3f5b0d2{id:03}");
        let mut marker = format!("slip:v1;id={id};created=2026-11-10T11:49:14+08:00");
        if archived {
            marker.push_str(";archived_at=2026-11-11T09:00:00+08:00");
        }
        let checkbox = if done { "x" } else { " " };
        format!("- [{checkbox}] 条目 {id} <!-- {marker} -->\n")
    }

    fn capacity_of(lines: &[String]) -> ArchivedCapacity {
        let text: String = lines.concat();
        archived_capacity(text.as_bytes())
    }

    #[test]
    fn empty_file_has_zero_capacity() {
        let capacity = archived_capacity(b"");
        assert_eq!(capacity.archived, 0);
        assert_eq!(capacity.archived_bytes, 0);
        assert_eq!(capacity.total_records, 0);
        assert_eq!(capacity.total_bytes, 0);
        // 空文件不能算出 NaN——除法前必须挡住 0。
        assert_eq!(capacity.archived_ratio(), 0.0);
    }

    #[test]
    fn no_archived_records() {
        let capacity = capacity_of(&[marker(1, false, false), marker(2, true, false)]);
        assert_eq!(capacity.total_records, 2);
        assert_eq!(capacity.archived, 0);
        assert_eq!(capacity.archived_bytes, 0);
    }

    #[test]
    fn counts_only_effective_archives() {
        // 三条：未完成、已完成但未归档、已完成且已归档。
        // 只有第三条算墓碑——中途那条"已完成但没归档"仍然在列表里。
        let lines = [
            marker(1, false, false),
            marker(2, true, false),
            marker(3, true, true),
        ];
        let capacity = capacity_of(&lines);
        assert_eq!(capacity.total_records, 3);
        assert_eq!(capacity.archived, 1);
        // 字节数应等于第三条的原始长度（含换行）。
        assert_eq!(capacity.archived_bytes, lines[2].len() as u64);
    }

    #[test]
    fn done_without_archived_at_is_not_a_tombstone() {
        // 已完成但没有 archived_at：不算归档。
        let capacity = capacity_of(&[marker(1, true, false)]);
        assert_eq!(capacity.archived, 0);
    }

    #[test]
    fn archived_at_without_done_is_not_a_tombstone() {
        // 未完成但残留 archived_at（外部编辑可能造成）：按 v11 §1.8 的算法，
        // 未完成时的残留归档时间**不隐藏条目**，因此也不算墓碑。
        let raw = "- [ ] 残留 <!-- slip:v1;id=019a9f3c-2b70-7d84-a1c6-8e3f5b0d2001;created=2026-11-10T11:49:14+08:00;archived_at=2026-11-11T09:00:00+08:00 -->\n";
        let capacity = archived_capacity(raw.as_bytes());
        assert_eq!(capacity.total_records, 1);
        assert_eq!(capacity.archived, 0);
    }

    #[test]
    fn all_archived_is_full_ratio() {
        let lines = [marker(1, true, true), marker(2, true, true)];
        let capacity = capacity_of(&lines);
        assert_eq!(capacity.archived, 2);
        // 整个文件都是墓碑 → 占比 1.0（没有空行/BOM 干扰时成立）。
        assert!((capacity.archived_ratio() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn unknown_keys_do_not_break_counting() {
        // 归档行里带未知键（未来版本写的）也要能正常统计。
        let raw = "- [x] 旧 <!-- slip:v1;zz=9;id=019a9f3c-2b70-7d84-a1c6-8e3f5b0d2001;created=2026-11-10T11:49:14+08:00;archived_at=2026-11-11T09:00:00+08:00 -->\n";
        let capacity = archived_capacity(raw.as_bytes());
        assert_eq!(capacity.archived, 1);
        assert_eq!(capacity.archived_bytes, raw.len() as u64);
    }

    #[test]
    fn invalid_lines_are_not_counted_as_archived() {
        // 解析不了的行不是墓碑（它是问题），不能混进"可清理"里让用户误删。
        let raw = "- [x] 坏的 <!-- slip:v1;id=not-a-uuid;created=2026-11-10T11:49:14+08:00;archived_at=2026-11-11T09:00:00+08:00 -->\n";
        let capacity = archived_capacity(raw.as_bytes());
        assert_eq!(capacity.archived, 0);
        assert_eq!(capacity.total_records, 0);
        // 但字节数仍然算进总文件大小，否则占比会失真。
        assert_eq!(capacity.total_bytes, raw.len() as u64);
    }
}
