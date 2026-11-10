//! 视图合成与发布（v11 §6.3）。
//!
//! 三条不可协商的性质：
//!
//! 1. **凡生成并发射新快照就递增 `ui_rev`**——store 结果、视图事件（DPI / 拖拽 /
//!    dock_side / 折叠）、运行时变化一视同仁。`ViewEvent` 没有 ACK，若沿用旧版本，
//!    前端 `s.ui_rev > last.ui_rev` 会把新几何**永久丢弃**。
//! 2. **`accept` 无条件接纳** store 结果。`content_rev` 相同不等于几何没变，
//!    不能把它当"整份快照没变"的去重键。
//! 3. **`GetSnapshot` 不递增**：它返回已保存的 `current`，是发布丢失后的补偿路径。
//!
//! 合成只依赖 `latest + view + runtime`，不携带陈旧内容；`emit` 失败由调用方记录，
//! **不回滚 `latest`**，也不把已提交的操作改成失败。

use std::collections::BTreeSet;

use slip_domain::{Slip, SlipId};
use slip_store::prefs::{DockSide, Prefs};
use slip_store::{Diagnostic, RecoveryState, StoreOutcome, StoreSnapshot, Warning};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::{HealthView, RecoveryView, RuntimeStatus, SlipView, UiSnapshot};

/// 显示器几何快照。
///
/// 只保留前端确实需要的整数几何：`f32` 会让 `Eq` 落空，也会把平台精度问题
/// 漏进视图层。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MonitorSnapshot {
    pub monitors: Vec<MonitorGeometry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorGeometry {
    pub name: String,
    pub width_px: u32,
    pub height_px: u32,
    /// 缩放比例以千分之一为单位（125% = 1250）。
    pub scale_milli: u32,
}

/// 视图事件。**没有 ACK**，因此必须自己推动版本号（v11 §6.3、I19）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ViewEvent {
    MonitorsChanged(MonitorSnapshot),
    DragEnd { id: SlipId, order: u32 },
    DockSideChanged(DockSide),
    CollapseToggled { id: SlipId },
}

/// 展示层状态。约定：**不进入 `StoreSnapshot`、不进 Markdown**。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PaperViewSet {
    /// 显式顺序（`Prefs.order` 的简化版）；未列出的条目按文档顺序跟在后面。
    pub order: Vec<SlipId>,
    pub collapsed: BTreeSet<SlipId>,
    pub dock_side: DockSide,
    pub monitors: MonitorSnapshot,
    /// 原样携带，**本层从不解释它**。
    ///
    /// 为什么它在这里：`Prefs` 是整体读、整体写的，而 `from_prefs` / `to_prefs`
    /// 是同一份偏好的两个方向。少了这个字段，任何一次拖拽或折叠触发的保存
    /// 都会把用户配置的热键**静默擦掉**。它可以被原样搬运，但不能被丢弃。
    pub hotkey: Option<String>,
}

impl PaperViewSet {
    /// 载入偏好时清理：去掉重复与悬空 ID，现有条目按文档顺序补到末尾（§1.8）。
    pub fn from_prefs(prefs: &Prefs, slips: &[Slip]) -> Self {
        let mut seen = BTreeSet::new();
        let mut order = Vec::with_capacity(slips.len());
        for id in &prefs.order {
            if slips.iter().any(|slip| slip.id == *id) && seen.insert(*id) {
                order.push(*id);
            }
        }
        for slip in slips {
            if seen.insert(slip.id) {
                order.push(slip.id);
            }
        }

        Self {
            order,
            collapsed: prefs
                .collapsed
                .iter()
                .copied()
                .filter(|id| slips.iter().any(|slip| slip.id == *id))
                .collect(),
            dock_side: prefs.dock_side,
            monitors: MonitorSnapshot::default(),
            hotkey: prefs.hotkey.clone(),
        }
    }

    pub fn to_prefs(&self) -> Prefs {
        Prefs {
            order: self.order.clone(),
            collapsed: self.collapsed.iter().copied().collect(),
            dock_side: self.dock_side,
            hotkey: self.hotkey.clone(),
        }
    }

    /// 当前可见顺序：先按 `order` 列出的现存条目，再按文档顺序补齐其余条目。
    ///
    /// `order` 里的悬空 ID（已被外部删除）被忽略，不会在列表里留下空洞。
    pub fn ordered<'a>(&self, slips: &'a [Slip]) -> Vec<&'a Slip> {
        let mut out: Vec<&Slip> = Vec::with_capacity(slips.len());
        let mut seen: BTreeSet<SlipId> = BTreeSet::new();
        for id in &self.order {
            if let Some(slip) = slips.iter().find(|slip| slip.id == *id)
                && seen.insert(*id)
            {
                out.push(slip);
            }
        }
        for slip in slips {
            if seen.insert(slip.id) {
                out.push(slip);
            }
        }
        out
    }

    /// 拖拽结束：把 `id` 移到当前可见顺序的第 `index` 位。
    ///
    /// 以可见顺序为基准重建 `order`，因此拖一次之后顺序就是用户看到的顺序。
    pub fn drag(&mut self, slips: &[Slip], id: SlipId, index: u32) {
        let mut order: Vec<SlipId> = self
            .ordered(slips)
            .into_iter()
            .map(|slip| slip.id)
            .collect();
        let Some(position) = order.iter().position(|candidate| *candidate == id) else {
            // 陈旧事件（条目已被删除）：不改顺序，避免写出悬空 ID。
            return;
        };
        let moved = order.remove(position);
        let index = (index as usize).min(order.len());
        order.insert(index, moved);
        self.order = order;
    }

    /// 返回切换后的折叠状态。
    pub fn toggle_collapsed(&mut self, id: SlipId) -> bool {
        if self.collapsed.remove(&id) {
            false
        } else {
            self.collapsed.insert(id);
            true
        }
    }
}

pub struct Presenter {
    latest: StoreSnapshot,
    view: PaperViewSet,
    runtime: RuntimeStatus,
    current: Option<UiSnapshot>,
    ui_rev: u64,
}

impl Presenter {
    /// 初始化：立即合成 `ui_rev = 1` 的完整快照（v11 §6.3）。
    pub fn new(initial: StoreSnapshot) -> Self {
        Self::with_view(initial, PaperViewSet::default())
    }

    pub fn with_view(initial: StoreSnapshot, view: PaperViewSet) -> Self {
        let mut presenter = Self {
            latest: initial,
            view,
            runtime: RuntimeStatus::Running,
            current: None,
            ui_rev: 0,
        };
        let _ = presenter.publish();
        presenter
    }

    /// 接纳 store 结果。**必须无条件替换 `latest`**，不按 `content_rev` 过滤。
    ///
    /// 业务错误同样携带正常快照，因此失败分支也走这里：健康状态、`needs_reload`、
    /// 诊断的变化必须被看见。
    pub fn accept(&mut self, outcome: &StoreOutcome) -> Option<&UiSnapshot> {
        // `content_rev` 只作"不回退"断言，绝不作为丢弃新快照的理由。
        debug_assert!(
            outcome.snapshot.content_rev >= self.latest.content_rev,
            "content_rev must not go backwards: {} -> {}",
            self.latest.content_rev.0,
            outcome.snapshot.content_rev.0
        );
        self.latest = outcome.snapshot.clone();
        self.publish()
    }

    /// 当前快照（供 `GetSnapshot` 补偿读取）。**不递增 `ui_rev`。**
    pub fn current(&self) -> Option<&UiSnapshot> {
        self.current.as_ref()
    }

    pub fn ui_rev(&self) -> u64 {
        self.ui_rev
    }

    pub fn view(&self) -> &PaperViewSet {
        &self.view
    }

    pub fn latest(&self) -> &StoreSnapshot {
        &self.latest
    }

    pub fn runtime(&self) -> &RuntimeStatus {
        &self.runtime
    }

    pub fn is_fatal(&self) -> bool {
        matches!(self.runtime, RuntimeStatus::Fatal { .. })
    }

    /// 切换运行时状态。切到 `Fatal` 时**保留最后内容**，只换状态并仍递增 `ui_rev`。
    pub fn set_runtime(&mut self, status: RuntimeStatus) {
        self.runtime = status;
        let _ = self.publish();
    }

    /// 应用一个视图事件：同样递增 `ui_rev` 并重新合成。
    pub fn apply_view_event(&mut self, event: &ViewEvent) -> Option<&UiSnapshot> {
        match event {
            ViewEvent::MonitorsChanged(monitors) => self.view.monitors = monitors.clone(),
            ViewEvent::DragEnd { id, order } => self.view.drag(&self.latest.slips, *id, *order),
            ViewEvent::DockSideChanged(side) => self.view.dock_side = *side,
            ViewEvent::CollapseToggled { id } => {
                self.view.toggle_collapsed(*id);
            }
        }
        self.publish()
    }

    pub fn monitors_changed(&mut self, monitors: MonitorSnapshot) -> Option<&UiSnapshot> {
        self.apply_view_event(&ViewEvent::MonitorsChanged(monitors))
    }

    pub fn drag_end(&mut self, id: SlipId, order: u32) -> Option<&UiSnapshot> {
        self.apply_view_event(&ViewEvent::DragEnd { id, order })
    }

    pub fn dock_side_changed(&mut self, side: DockSide) -> Option<&UiSnapshot> {
        self.apply_view_event(&ViewEvent::DockSideChanged(side))
    }

    /// 切换折叠并发布；返回切换后的折叠状态。
    pub fn toggle_collapse(&mut self, id: SlipId) -> bool {
        let collapsed = self.view.toggle_collapsed(id);
        let _ = self.publish();
        collapsed
    }

    /// 合成并保存新快照，`ui_rev` 先递增再合成，保证新快照永远比上一份大。
    fn publish(&mut self) -> Option<&UiSnapshot> {
        self.ui_rev = self.ui_rev.saturating_add(1);
        let snapshot = self.compose(self.ui_rev);
        self.current = Some(snapshot);
        self.current.as_ref()
    }

    fn compose(&self, ui_rev: u64) -> UiSnapshot {
        let health = &self.latest.health;
        UiSnapshot {
            content_rev: self.latest.content_rev.0,
            ui_rev,
            slips: self
                .view
                .ordered(&self.latest.slips)
                .into_iter()
                .map(slip_view)
                .collect(),
            health: HealthView {
                writable: health.is_writable(),
                warnings: health.warnings.iter().map(warning_text).collect(),
            },
            needs_reload: self.latest.needs_reload,
            recovery: recovery_view(&self.latest.recovery),
            diagnostics: self
                .latest
                .diagnostics
                .iter()
                .map(diagnostic_text)
                .collect(),
            runtime: self.runtime.clone(),
        }
    }
}

fn slip_view(slip: &Slip) -> SlipView {
    SlipView {
        id: slip.id.to_canonical_string(),
        text: slip.text.clone(),
        done: slip.done,
        // 有效归档的唯一算法（v11 §1.8）：未完成时的残留 `archived_at` 不隐藏条目。
        archived: slip.done && slip.archived_at.is_some(),
        locked: slip.locked,
        created_at: rfc3339(slip.created_at),
        done_at: slip.done_at.map(rfc3339),
    }
}

/// 时间以 RFC3339 下发，不把后端时间类型泄漏到 TS 边界。
///
/// 无法格式化的时间在写盘前就已被领域层拒绝，这里只能给出空串而不是 panic。
fn rfc3339(at: OffsetDateTime) -> String {
    at.format(&Rfc3339).unwrap_or_default()
}

fn warning_text(warning: &Warning) -> String {
    match warning {
        Warning::BackupFailed => "backupFailed".to_owned(),
        Warning::UnknownField { line } => format!("unknownField:{line}"),
        Warning::DurabilityUnconfirmed => "durabilityUnconfirmed".to_owned(),
    }
}

fn diagnostic_text(diagnostic: &Diagnostic) -> String {
    match diagnostic.line {
        Some(line) => format!("line {line}: {}", diagnostic.message),
        None => diagnostic.message.clone(),
    }
}

fn recovery_view(recovery: &RecoveryState) -> RecoveryView {
    match recovery {
        RecoveryState::Normal => RecoveryView::Normal,
        RecoveryState::Isolated { corrupt_path, .. } => RecoveryView::Isolated {
            corrupt_path: corrupt_path.display().to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use slip_domain::{ContentRev, Slip};
    use slip_store::{Availability, Health, StoreError, StoreSnapshot};

    #[test]
    fn view_roundtrip_never_drops_the_hotkey() {
        // 回归：`Prefs` 是整体写的，视图层少带一个字段就等于每次拖拽/折叠
        // 都把用户配置的热键擦掉。这个测试锁的就是"不许丢"。
        let prefs = Prefs {
            hotkey: Some(String::from("ctrl+alt+h")),
            ..Prefs::default()
        };

        let view = PaperViewSet::from_prefs(&prefs, &[]);

        assert_eq!(view.to_prefs().hotkey, Some(String::from("ctrl+alt+h")));
    }

    #[test]
    fn from_prefs_drops_dangling_and_duplicate_ids() {
        let a = slip("a", false);
        let b = slip("b", false);
        let dangling = id_of("00000000-0000-7000-8000-000000000000");
        let prefs = Prefs {
            order: vec![b.id, b.id, dangling, a.id],
            collapsed: vec![dangling, a.id],
            ..Prefs::default()
        };

        let view = PaperViewSet::from_prefs(&prefs, &[a.clone(), b.clone()]);

        assert_eq!(view.order, vec![b.id, a.id]);
        assert_eq!(view.collapsed, BTreeSet::from([a.id]));
    }

    fn slip(text: &str, done: bool) -> Slip {
        Slip {
            id: SlipId::new_v7(),
            text: text.to_owned(),
            done,
            created_at: OffsetDateTime::now_utc(),
            done_at: None,
            archived_at: None,
            locked: false,
        }
    }

    fn snapshot(rev: u64, slips: Vec<Slip>) -> StoreSnapshot {
        StoreSnapshot {
            content_rev: ContentRev(rev),
            slips,
            health: Health::read_write(),
            needs_reload: false,
            recovery: RecoveryState::Normal,
            diagnostics: Vec::new(),
        }
    }

    fn id_of(text: &str) -> SlipId {
        SlipId::parse_strict(text).unwrap()
    }

    #[test]
    fn new_publishes_first_snapshot_with_ui_rev_one() {
        let presenter = Presenter::new(snapshot(1, vec![slip("a", false)]));
        let current = presenter.current().expect("initial snapshot");
        assert_eq!(current.ui_rev, 1);
        assert_eq!(current.content_rev, 1);
        assert_eq!(presenter.ui_rev(), 1);
        assert_eq!(current.slips.len(), 1);
    }

    #[test]
    fn accept_increments_ui_rev_even_when_content_rev_is_unchanged() {
        // 同 content_rev 的另一份快照（例如健康/诊断变化）**不能**被丢弃。
        let mut presenter = Presenter::new(snapshot(1, vec![slip("a", false)]));
        let first = presenter.current().unwrap().ui_rev;

        let mut outcome = StoreOutcome::ok(snapshot(1, vec![slip("a", false)]));
        outcome.snapshot.needs_reload = true;
        let second = presenter.accept(&outcome).expect("snapshot").ui_rev;
        assert_eq!(second, first + 1, "同 content_rev 也必须发布新版本");

        let current = presenter.current().unwrap();
        assert_eq!(current.ui_rev, second);
        assert!(current.needs_reload);
        assert!(presenter.latest().needs_reload);
    }

    #[test]
    fn accept_trace_is_in_order() {
        // I17 的 presenter 侧：接纳顺序 == 执行顺序（由调用方顺序决定）。
        let mut presenter = Presenter::new(snapshot(1, Vec::new()));
        let mut accepted = Vec::new();
        for (rev, text) in [(2u64, "a"), (3, "b")] {
            let outcome = StoreOutcome::ok(snapshot(rev, vec![slip(text, false)]));
            let current = presenter.accept(&outcome).expect("snapshot");
            accepted.push((
                current.content_rev,
                current.slips[0].text.clone(),
                current.ui_rev,
            ));
        }
        assert_eq!(
            accepted
                .iter()
                .map(|(rev, text, _)| (*rev, text.as_str()))
                .collect::<Vec<_>>(),
            vec![(2, "a"), (3, "b")]
        );
        // ui_rev 严格递增：1 → 2 → 3。
        assert_eq!(
            accepted.iter().map(|(_, _, ui)| *ui).collect::<Vec<_>>(),
            vec![2, 3]
        );
    }

    #[test]
    fn view_events_increment_ui_rev() {
        // v11 修的 bug：视图事件没有 ACK，若不递增版本号，前端会把新几何永久丢弃。
        let mut presenter = Presenter::new(snapshot(1, Vec::new()));
        let before = presenter.ui_rev();

        presenter.monitors_changed(MonitorSnapshot {
            monitors: vec![MonitorGeometry {
                name: "primary".to_owned(),
                width_px: 2560,
                height_px: 1440,
                scale_milli: 1250,
            }],
        });
        assert_eq!(presenter.ui_rev(), before + 1, "DPI 变化必须递增 ui_rev");

        let id = SlipId::new_v7();
        presenter.drag_end(id, 0);
        assert_eq!(presenter.ui_rev(), before + 2, "拖拽结束必须递增 ui_rev");

        presenter.dock_side_changed(DockSide::Right);
        assert_eq!(presenter.ui_rev(), before + 3, "dock_side 必须递增 ui_rev");

        presenter.toggle_collapse(id);
        assert_eq!(presenter.ui_rev(), before + 4, "折叠必须递增 ui_rev");

        // 版本号与已保存快照一致。
        assert_eq!(presenter.current().unwrap().ui_rev, presenter.ui_rev());
        assert_eq!(presenter.current().unwrap().content_rev, 1);
    }

    #[test]
    fn view_event_after_failed_emit_still_bumps_version() {
        // §9.1 第 8 条：ui_rev=21 之后 emit 失败，再来一次 DPI 事件 → 仍要 > 21。
        let mut presenter = Presenter::new(snapshot(1, Vec::new()));
        while presenter.ui_rev() < 21 {
            let _ = presenter.accept(&StoreOutcome::ok(snapshot(1, Vec::new())));
        }
        assert_eq!(presenter.ui_rev(), 21);
        // emit 失败由调用方处理，presenter 不会回滚，也不需要知道。
        let after_view = presenter
            .monitors_changed(MonitorSnapshot::default())
            .unwrap();
        assert!(after_view.ui_rev > 21);
        assert_eq!(after_view.content_rev, 1);
    }

    #[test]
    fn current_does_not_bump_ui_rev() {
        let mut presenter = Presenter::new(snapshot(1, vec![slip("a", false)]));
        let _ = presenter.accept(&StoreOutcome::ok(snapshot(2, vec![slip("a", false)])));
        let before = presenter.ui_rev();
        let a = presenter.current().cloned().unwrap();
        let b = presenter.current().cloned().unwrap();
        assert_eq!(a, b);
        assert_eq!(presenter.ui_rev(), before);
    }

    #[test]
    fn set_runtime_fatal_keeps_content_and_bumps_rev() {
        let mut presenter = Presenter::new(snapshot(1, vec![slip("a", false)]));
        let before = presenter.ui_rev();
        presenter.set_runtime(RuntimeStatus::Fatal {
            reason: "boom".to_owned(),
        });
        let current = presenter.current().unwrap();
        assert!(current.runtime.is_fatal());
        assert_eq!(current.ui_rev, before + 1);
        assert_eq!(current.content_rev, 1);
        assert_eq!(current.slips.len(), 1, "Fatal 冻结最后已接纳内容");
    }

    #[test]
    fn maps_store_snapshot_to_view() {
        let mut done = slip("done", true);
        done.done_at = Some(OffsetDateTime::now_utc());
        done.archived_at = Some(OffsetDateTime::now_utc());
        // 未完成但残留归档时间：按 v11 §1.8 不得算作已归档。
        let mut residual = slip("residual", false);
        residual.archived_at = Some(OffsetDateTime::now_utc());

        let mut health = Health::degraded();
        health.warnings.insert(Warning::BackupFailed);
        health.warnings.insert(Warning::UnknownField { line: 7 });
        health.warnings.insert(Warning::DurabilityUnconfirmed);

        let store = StoreSnapshot {
            content_rev: ContentRev(9),
            slips: vec![done, residual],
            health,
            needs_reload: true,
            recovery: RecoveryState::Isolated {
                corrupt_path: std::path::PathBuf::from("C:/data/inbox.md"),
                isolated_at: OffsetDateTime::now_utc(),
            },
            diagnostics: vec![
                Diagnostic {
                    line: Some(3),
                    message: "bad record".to_owned(),
                },
                Diagnostic {
                    line: None,
                    message: "generic".to_owned(),
                },
            ],
        };
        let presenter = Presenter::new(store);
        let current = presenter.current().unwrap();

        assert_eq!(current.content_rev, 9);
        assert!(!current.health.writable, "Degraded 不可写");
        assert_eq!(
            current.health.warnings.iter().cloned().collect::<Vec<_>>(),
            vec![
                "backupFailed".to_owned(),
                "durabilityUnconfirmed".to_owned(),
                "unknownField:7".to_owned()
            ]
        );
        assert!(current.needs_reload);
        assert_eq!(
            current.recovery,
            RecoveryView::Isolated {
                corrupt_path: "C:/data/inbox.md".to_owned()
            }
        );
        assert_eq!(
            current.diagnostics,
            vec!["line 3: bad record".to_owned(), "generic".to_owned()]
        );

        assert!(current.slips[0].done);
        assert!(
            current.slips[0].archived,
            "done && archived_at 才算有效归档"
        );
        assert!(current.slips[0].done_at.is_some());
        assert!(!current.slips[1].done);
        assert!(!current.slips[1].archived, "未完成条目不因残留归档时间隐藏");
        // 时间是 RFC3339 字符串。
        assert!(
            current.slips[0].created_at.ends_with('Z') || current.slips[0].created_at.contains('+')
        );
    }

    #[test]
    fn business_error_outcome_is_accepted_too() {
        // 失败必须携带可发布快照：健康/诊断变化要被看见。
        let mut presenter = Presenter::new(snapshot(1, vec![slip("a", false)]));
        let mut degraded = snapshot(1, vec![slip("a", false)]);
        degraded.health = Health {
            availability: Availability::Degraded,
            warnings: BTreeSet::new(),
        };
        let outcome = StoreOutcome::failed(degraded, StoreError::ReadOnly);
        let current = presenter.accept(&outcome).unwrap();
        assert!(!current.health.writable);
        assert_eq!(presenter.latest().content_rev, ContentRev(1));
    }

    #[test]
    fn paper_view_order_follows_prefs_then_document_order() {
        let a = slip("a", false);
        let b = slip("b", false);
        let c = slip("c", false);
        let (id_a, id_b, id_c) = (a.id, b.id, c.id);
        let prefs = Prefs {
            order: vec![id_c, id_a],
            collapsed: vec![id_b],
            dock_side: DockSide::Bottom,
            hotkey: None,
        };
        let slips = vec![a, b, c];
        let view = PaperViewSet::from_prefs(&prefs, &slips);
        let mut presenter = Presenter::with_view(snapshot(1, slips), view);
        assert_eq!(
            presenter
                .current()
                .unwrap()
                .slips
                .iter()
                .map(|slip| slip.text.as_str())
                .collect::<Vec<_>>(),
            vec!["c", "a", "b"]
        );

        // 拖拽 a 到末尾。
        presenter.drag_end(id_a, 2);
        assert_eq!(
            presenter
                .current()
                .unwrap()
                .slips
                .iter()
                .map(|slip| slip.text.as_str())
                .collect::<Vec<_>>(),
            vec!["c", "b", "a"]
        );
        assert_eq!(presenter.view().to_prefs().collapsed, prefs.collapsed);

        // 陈旧拖拽事件（条目已不存在）不改顺序。
        let before = presenter.ui_rev();
        presenter.drag_end(id_of("00000000-0000-7000-8000-000000000000"), 0);
        assert_eq!(presenter.ui_rev(), before + 1);
        assert_eq!(
            presenter
                .current()
                .unwrap()
                .slips
                .iter()
                .map(|slip| slip.text.as_str())
                .collect::<Vec<_>>(),
            vec!["c", "b", "a"]
        );
        let _ = id_c;
    }
}
