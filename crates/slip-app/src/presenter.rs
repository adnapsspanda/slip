//! 视图合成与发布（v11 §6.3）。
//!
//! 三条不可协商的性质：
//!
//! 1. **凡生成并发射新快照就递增 `ui_rev`**——store 结果与运行时变化一视同仁。
//!    前端用 `s.ui_rev > last.ui_rev` 丢弃过期快照；版本号不前进，新状态就被
//!    **永久丢弃**。
//! 2. **`accept` 无条件接纳** store 结果。`content_rev` 相同不等于几何没变，
//!    不能把它当"整份快照没变"的去重键。
//! 3. **`GetSnapshot` 不递增**：它返回已保存的 `current`，是发布丢失后的补偿路径。
//!
//! 合成只依赖 `latest + view + runtime`，不携带陈旧内容；`emit` 失败由调用方记录，
//! **不回滚 `latest`**，也不把已提交的操作改成失败。

use slip_domain::Slip;
use slip_store::prefs::Prefs;
use slip_store::{Diagnostic, RecoveryState, StoreOutcome, StoreSnapshot, Warning};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::{HealthView, RecoveryView, RuntimeStatus, SlipView, UiSnapshot};

/// 展示层里唯一需要持久化的东西。
///
/// 早先这里还有 `order` / `collapsed` / `dock_side` / `monitors`，以及配套的
/// `ViewEvent` 通路与 `dispatch` 里的偏好去抖机制。但前端**从未产生过任何视图事件**
/// ——整条路径是死的，却让 `dispatch` 主循环多背了一个 `select!` 分支与一套
/// dirty/due 状态机。已整体删除。
///
/// 将来若要重新引入拖拽 / 折叠，必须先给出 UI 入口与验收用例，
/// 而不是先把后端通路加回来。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PaperViewSet {
    /// 原样携带，**本层从不解释它**：`Prefs` 是整体读写的，少带一个字段就等于
    /// 每次保存都把用户配置的热键静默擦掉。
    pub hotkey: Option<String>,
}

impl PaperViewSet {
    pub fn from_prefs(prefs: &Prefs) -> Self {
        Self {
            hotkey: prefs.hotkey.clone(),
        }
    }

    pub fn to_prefs(&self) -> Prefs {
        Prefs {
            hotkey: self.hotkey.clone(),
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
            // 列表顺序就是文档顺序：显式 `order` 与拖拽通路已随死视图路径删除。
            slips: self.latest.slips.iter().map(slip_view).collect(),
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
        pinned: slip.pinned,
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
    use std::collections::BTreeSet;

    use super::*;
    use slip_domain::{ContentRev, Slip, SlipId};
    use slip_store::{Availability, Health, StoreError, StoreSnapshot};

    #[test]
    fn view_roundtrip_never_drops_the_hotkey() {
        // 回归：`Prefs` 是整体写的，视图层少带一个字段就等于每次拖拽/折叠
        // 都把用户配置的热键擦掉。这个测试锁的就是"不许丢"。
        let prefs = Prefs {
            hotkey: Some(String::from("ctrl+alt+h")),
        };

        let view = PaperViewSet::from_prefs(&prefs);

        assert_eq!(view.to_prefs().hotkey, Some(String::from("ctrl+alt+h")));
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
            pinned: false,
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
}
