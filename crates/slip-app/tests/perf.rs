//! `slip-app` 的快照序列化基准（全部 `#[ignore]`，必须显式运行）。
//!
//! 运行方式：
//!
//! ```text
//! cargo test --release -p slip-app --test perf -- --ignored --nocapture
//! ```
//!
//! 为什么单独测：10,000 条是文档宣称的目标规模。每次变更后前端都用
//! `get_snapshot` 全量拉取，快照的 JSON 体积与序列化耗时是交互延迟的固定成本。
//! 这里只**测量**，不设紧门限——协议要不要改，先看数字。

use std::collections::BTreeSet;
use std::time::Instant;

use slip_app::{HealthView, RecoveryView, RuntimeStatus, SlipView, UiSnapshot};

fn snapshot(count: usize) -> UiSnapshot {
    let slips = (0..count)
        .map(|index| SlipView {
            id: format!("019a9f3c-2b70-7d84-a1c6-{index:012x}"),
            text: format!("任务 {index:05} 的正文内容大约二十五个汉字"),
            done: index % 3 == 0,
            archived: index % 7 == 0,
            locked: false,
            pinned: index % 11 == 0,
            created_at: String::from("2026-11-12T09:00:00+08:00"),
            done_at: None,
        })
        .collect();
    UiSnapshot {
        content_rev: 1,
        ui_rev: 1,
        slips,
        health: HealthView {
            writable: true,
            warnings: BTreeSet::new(),
        },
        needs_reload: false,
        recovery: RecoveryView::Normal,
        diagnostics: Vec::new(),
        runtime: RuntimeStatus::Running,
    }
}

#[test]
#[ignore = "性能测量，需要 --ignored --nocapture"]
fn snapshot_serialization_at_10k() {
    let snapshot = snapshot(10_000);
    let started = Instant::now();
    let json = serde_json::to_vec(&snapshot).expect("序列化快照");
    let elapsed = started.elapsed();
    println!(
        "[snapshot] 10000 条：{} 字节（{:.2} MiB），序列化 {:?}",
        json.len(),
        json.len() as f64 / 1024.0 / 1024.0,
        elapsed
    );
    // 这个门限只抓"格式膨胀到不可收拾"，不用于性能打分。
    assert!(
        json.len() < 8 * 1024 * 1024,
        "10k 快照超过 8 MiB：{} 字节",
        json.len()
    );
}
