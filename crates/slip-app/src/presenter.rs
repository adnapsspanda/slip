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
use slip_store::{Diagnostic, RecoveryState, StoreOutcome, StoreSnapshot, Warning};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::{HealthView, RecoveryView, RuntimeStatus, SlipView, UiSnapshot};

pub struct Presenter {
    latest: StoreSnapshot,
    runtime: RuntimeStatus,
    current: Option<UiSnapshot>,
    ui_rev: u64,
}

impl Presenter {
    /// 初始化：立即合成 `ui_rev = 1` 的完整快照（v11 §6.3）。
    pub fn new(initial: StoreSnapshot) -> Self {
        let mut presenter = Self {
            latest: initial,
            runtime: RuntimeStatus::Running,
            current: None,
            ui_rev: 0,
        };
        let _ = presenter.publish();
        presenter
    }

    /// 接纳 store 结果。**正常情况下无条件替换 `latest`**，不按 `content_rev` 过滤。
    ///
    /// 业务错误同样携带正常快照，因此失败分支也走这里：健康状态、`needs_reload`、
    /// 诊断的变化必须被看见。
    pub fn accept(&mut self, outcome: &StoreOutcome) -> Option<&UiSnapshot> {
        // `content_rev` 回退是 store 违反契约的信号。以前只有 `debug_assert!`，
        // release 构建里它会被编译掉：一份降版本的旧快照会静默覆盖更新的内容，
        // 而且前后端全部版本比较从此失真。这里改成**运行期**判定——丢弃这份
        // 降版本快照、保留 `latest`，并把运行时置为 Fatal 让错误显式可见。
        //
        // 仍然调用 `publish()`：Fatal 只换状态、保留最后已接纳内容，但每次生成
        // 新快照都必须递增 `ui_rev`（§6.3），否则前端会永久停在旧状态上。
        if outcome.snapshot.content_rev < self.latest.content_rev {
            self.runtime = RuntimeStatus::Fatal {
                reason: format!(
                    "content_rev went backwards: {} -> {}",
                    self.latest.content_rev.0, outcome.snapshot.content_rev.0
                ),
            };
            return self.publish();
        }
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
    fn accepting_a_regressed_content_rev_drops_it_and_goes_fatal() {
        // release 构建里旧的 `debug_assert!` 会被编译掉：store 若回退版本号，
        // 降版本的旧快照会静默覆盖更新的内容。这条钉住运行期行为——丢弃它、
        // 保留 `latest`、进入 Fatal，且仍然前进 `ui_rev`。
        let mut presenter = Presenter::new(snapshot(5, vec![slip("newer", false)]));
        let before = presenter.ui_rev();

        let regressed = StoreOutcome::ok(snapshot(4, vec![slip("older", false)]));
        let current = presenter.accept(&regressed).expect("snapshot");

        assert_eq!(current.ui_rev, before + 1, "Fatal 也必须发布新 ui_rev");
        assert_eq!(current.content_rev, 5, "降版本快照必须被丢弃");
        assert_eq!(current.slips.len(), 1);
        assert_eq!(current.slips[0].text, "newer");
        assert!(presenter.is_fatal());
        assert_eq!(presenter.latest().content_rev, ContentRev(5));
        assert_eq!(presenter.latest().slips[0].text, "newer");
        let reason = match presenter.runtime() {
            RuntimeStatus::Fatal { reason } => reason.as_str(),
            RuntimeStatus::Running => panic!("应当进入 Fatal"),
        };
        assert_eq!(reason, "content_rev went backwards: 5 -> 4");
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
