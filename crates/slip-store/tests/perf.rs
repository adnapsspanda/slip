//! `slip-store` 性能基线测量（全部 `#[ignore]`，必须显式运行）。
//!
//! 运行方式：
//!
//! ```text
//! cargo test -p slip-store --test perf -- --ignored --nocapture
//! ```
//!
//! 这些测试的性质是**测量**，不是验收：走真实临时目录、真实文件、真实文法，
//! 把数字打到 stdout，供文档直接引用；同时用**极宽松**的门限兜住灾难性回归
//! （例如格式改动让解析退化成 O(n²)、或某次写入退化成几百毫秒）。
//!
//! 门限为什么这么宽：测量机常常同时在跑别的编译任务，单次耗时的抖动可以达到
//! 一个数量级。门限的作用是"抓住明显坏掉"，不是给实现打分，所以宁可宽一百倍，
//! 也不能因为一次调度抖动误报。
//!
//! 注意：本文件只依赖公开 API；不为了让测试好写去放宽任何可见性。

use std::path::Path;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use slip_domain::{Mutation, Patch, Slip, SlipId};
use slip_store::{MarkdownStore, Store};
use time::OffsetDateTime;
use time::macros::datetime;

// ---------------------------------------------------------------------------
// 门限（宽松，但仍然存在，让测试有回归价值）
// ---------------------------------------------------------------------------

/// 1000 行文件的启动解析（`new` + `snapshot`）上限：50 ms。
///
/// 实测 release 构建 0.53 ms，留约 90 倍余量。原来的门限是 1 秒——比实测慢 1800 倍，
/// 那条断言实际上只能抓住"完全崩掉"，抓不住渐进劣化。
const STARTUP_1000_LIMIT_MS: f64 = 50.0;

/// 10000 行文件的启动解析上限：100 ms（矩阵 R72 的验收线）。
///
/// 实测 release 构建 6.37 ms（new 6.10 + snapshot 0.27），留约 16 倍余量。
/// 原来给的是 2 秒，比实测慢 300 倍——等于没守。这条线的意义是：一旦有人把解析写成
/// O(n²)，或在热路径里加了一次额外全量扫描，它会立刻红。
const STARTUP_10000_LIMIT_MS: f64 = 100.0;

/// 单条写入 p50 上限：30 ms。
///
/// 实测（debug 构建、1000 条文件）约 14–15 ms，主要花在整份文件重写 + 备份 +
/// fsync + 目录同步上；给到 30 ms 是约两倍余量。只有写入路径整体劣化才会撞线。
const WRITE_P50_LIMIT_US: f64 = 30_000.0;

/// 单条写入 p95 上限：50 ms。跨过这条线说明写入路径退化到用户可感知的卡顿。
const WRITE_P95_LIMIT_US: f64 = 50_000.0;

/// 单条写入 max 上限：500 ms。只用来抓"偶发一次就卡死 UI"的灾难，
/// 允许一两个样本被系统调度或杀毒软件抓走。
const WRITE_MAX_LIMIT_US: f64 = 500_000.0;

/// 10000 条时 `inbox.md` 的体积上限：4 MiB（实测约 1.1 MiB）。
/// 撞上说明每条记录被写胖了（例如重复 marker、时间格式变长）。
const SIZE_LIMIT_BYTES: u64 = 4 * 1024 * 1024;

/// 平均每条字节数的合理区间。下界防"字段被静默丢弃"，上界防"格式膨胀"。
const AVG_RECORD_MIN_BYTES: f64 = 64.0;
const AVG_RECORD_MAX_BYTES: f64 = 256.0;

// ---------------------------------------------------------------------------
// 夹具：真实文件、真实文法
// ---------------------------------------------------------------------------

/// 种子行的 `created` 值：写死，保证每次运行的文件内容逐字节可比。
fn created() -> OffsetDateTime {
    datetime!(2026-11-10 11:49:14 +08:00)
}

/// 与 [`SlipId`] 写入文件后完全一致的规范文本形式（小写连字符）。
const CREATED_TEXT: &str = "2026-11-10T11:49:14+08:00";

/// 生成 `count` 条**合法**记录行（不含终止符）。
///
/// 每条都用真实的 `SlipId::new_v7()`：既保证文法可解析，也顺带验证
/// "UUIDv7 批量生成不会撞号"——撞号会让整份文件被判为 degraded。
fn seed_lines(count: usize) -> Vec<String> {
    (0..count)
        .map(|index| {
            let id = SlipId::new_v7().to_canonical_string();
            format!("- [ ] 任务 {index:05} <!-- slip:v1;id={id};created={CREATED_TEXT} -->")
        })
        .collect()
}

/// 把行写进真实文件，带结尾换行（手工编辑过的文件通常长这样）。
///
/// 返回写入的字节数。
fn write_inbox(path: &Path, lines: &[String]) -> u64 {
    let mut body = lines.join("\n");
    body.push('\n');
    std::fs::write(path, body.as_bytes()).expect("写入 inbox.md");
    body.len() as u64
}

fn vault() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("slip-perf-")
        .tempdir()
        .expect("创建临时 vault")
}

/// 把一个 `Append` 用的新纸条构造出来（构造本身不计入耗时）。
fn new_slip(text: &str) -> Slip {
    Slip {
        id: SlipId::new_v7(),
        text: text.to_owned(),
        done: false,
        created_at: created(),
        done_at: None,
        archived_at: None,
        locked: false,
        pinned: false,
    }
}

// ---------------------------------------------------------------------------
// 串行化：这些测试在比"谁快"，并行跑会互相抢盘并让 stdout 交错
// ---------------------------------------------------------------------------

/// 进程内互斥量。`cargo test` 默认多线程并行跑测试，若不串行化，
/// 四个表格会交织在一起、数字也会被彼此的 I/O 抬上去——那样的输出既没法贴进
/// 文档，也不是想测的东西。锁中毒时照常继续（panicking 的测试已经在别处报告过）。
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

// ---------------------------------------------------------------------------
// 统计与打印
// ---------------------------------------------------------------------------

/// 一组以微秒计的耗时样本。
fn push_micros(samples: &mut Vec<f64>, elapsed: Duration) {
    samples.push(elapsed.as_secs_f64() * 1_000_000.0);
}

/// 最近秩百分位（样本已升序）。
///
/// 不插值：n=100 时 p50 就是第 50 个样本，读数更好解释。
fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let rank = (p / 100.0 * sorted.len() as f64).ceil().max(1.0) as usize;
    sorted[rank.min(sorted.len()) - 1]
}

fn mean(samples: &[f64]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    samples.iter().sum::<f64>() / samples.len() as f64
}

/// 打印一行延迟统计表（单位 µs），并返回 (p50, p95, max)。
fn report_latency(label: &str, mut samples: Vec<f64>) -> (f64, f64, f64) {
    samples.sort_by(f64::total_cmp);
    let p50 = percentile(&samples, 50.0);
    let p95 = percentile(&samples, 95.0);
    let max = samples.last().copied().unwrap_or(0.0);

    println!("{label}");
    println!(
        "  {:<6}{:>12}{:>12}{:>12}{:>12}",
        "n", "p50 µs", "p95 µs", "max µs", "mean µs"
    );
    println!(
        "  {:<6}{:>12.1}{:>12.1}{:>12.1}{:>12.1}",
        samples.len(),
        p50,
        p95,
        max,
        mean(&samples)
    );
    (p50, p95, max)
}

fn banner() {
    println!();
    println!("======================= slip-store 性能实测 =======================");
    // 数字必须在正确的语境里被引用：`cargo test` 默认是未优化的 debug 构建，
    // 这里的毫秒数因此偏保守；release 构建会显著更快。门限也是按 debug 定的。
    // 不能写死：同一份代码在 `--release` 下数字差好几倍，标签骗人比没标签更糟。
    println!(
        "  构建配置：{} | 时间来自 std::time::Instant | 真实临时目录 + fsync",
        if cfg!(debug_assertions) {
            "debug（未优化）"
        } else {
            "release（优化）"
        }
    );
    println!("  注意：`cargo test` 默认并行；本文件用互斥量串行化，避免表格交错与抢占 I/O。");
    println!();
}

fn pass(label: &str) {
    println!("  [PASS] {label}");
    println!();
}

// ---------------------------------------------------------------------------
// 1. 启动解析：真实存在的 inbox.md，1000 行 / 10000 行
// ---------------------------------------------------------------------------

/// 测量"文件已经存在时打开 store 并把内容解析成快照"要多久。
///
/// 证明的是**启动成本是线性的、且远低于人能感知的阈值**：这个数字直接决定
/// 应用冷启动到第一帧可用内容之间的空白时长。`new()` 里做的是读盘 + 解析 +
/// hash，`snapshot()` 只是把已解析的记录克隆出来，所以两列拆开打印——
/// 它们分别回答"解析贵不贵"和"发布快照贵不贵"。
#[test]
#[ignore = "性能测量，需要 --ignored --nocapture"]
fn startup_parse_existing_inbox_for_1000_and_10000_lines() {
    let _guard = serial();
    banner();
    println!("[1] 启动解析：文件已存在时 MarkdownStore::new(...).snapshot()");
    println!(
        "  {:<7}{:>12}{:>10}{:>13}{:>14}{:>12}",
        "lines", "bytes", "KB", "new() ms", "snapshot ms", "total ms"
    );

    for count in [1_000usize, 10_000usize] {
        let dir = vault();
        let path = dir.path().join("inbox.md");
        let bytes = write_inbox(&path, &seed_lines(count));

        // 解析耗时：new() 内部读盘并解析整份文件。
        let t_new = Instant::now();
        let store = MarkdownStore::new(&path);
        let new_ms = t_new.elapsed().as_secs_f64() * 1_000.0;

        // 快照耗时：把已解析的记录发布给调用方。
        let t_snap = Instant::now();
        let snapshot = store.snapshot();
        let snap_ms = t_snap.elapsed().as_secs_f64() * 1_000.0;
        let total_ms = new_ms + snap_ms;

        // 先确认测的是"成功解析"，否则数字再小也没有意义。
        assert!(
            snapshot.health.is_writable(),
            "{count} 行种子文件被判为不可写，格式或 ID 生成有问题：{:?}",
            snapshot.diagnostics
        );
        assert_eq!(
            snapshot.slips.len(),
            count,
            "{count} 行必须解析出 {count} 条"
        );
        assert_eq!(
            snapshot.slips[0].text, "任务 00000",
            "首条文本与种子不一致，测的不是预期文件"
        );

        println!(
            "  {:<7}{:>12}{:>10.1}{:>13.2}{:>14.2}{:>12.2}",
            count,
            bytes,
            bytes as f64 / 1024.0,
            new_ms,
            snap_ms,
            total_ms
        );

        let limit_ms = if count == 1_000 {
            STARTUP_1000_LIMIT_MS
        } else {
            STARTUP_10000_LIMIT_MS
        };
        println!("        门限：{limit_ms:.0} ms（{count} 行）");
        assert!(
            total_ms < limit_ms,
            "{count} 行启动解析耗时 {total_ms:.2} ms，超过门限 {limit_ms:.0} ms；\
             解析路径很可能退化成了非线性"
        );
        // `new()` 必须是解析成本的大头，`snapshot()` 只是克隆——
        // 若 `snapshot()` 反而更慢，说明快照在偷偷重新解析或读盘。
        assert!(
            snap_ms <= new_ms.max(5.0),
            "snapshot() 比 new() 还慢（{snap_ms:.2} ms vs {new_ms:.2} ms），\
             快照路径可能在做额外工作"
        );
    }

    pass("启动解析满足门限");
}

// ---------------------------------------------------------------------------
// 2. 单条追加延迟
// ---------------------------------------------------------------------------

/// 测量在已有 1000 条的文件上连续追加 100 条的单次延迟。
///
/// 证明的是**"记一条"的交互延迟**：用户敲下回车到内容落盘。这里的实现是整份
/// 文件重写 + 备份 + fsync + rename，所以单次成本随文件长度增长——这条基线就是
/// 判断"1000 条时用户还感觉不到卡"的依据。
#[test]
#[ignore = "性能测量，需要 --ignored --nocapture"]
fn append_latency_on_1000_line_inbox() {
    let _guard = serial();
    banner();
    let dir = vault();
    let path = dir.path().join("inbox.md");
    write_inbox(&path, &seed_lines(1_000));

    let store = MarkdownStore::new(&path);
    assert_eq!(store.snapshot().slips.len(), 1_000, "种子文件必须可解析");

    let mut samples = Vec::with_capacity(100);
    for index in 0..100 {
        // 构造不计入耗时：只测 store 自己的写入成本。
        let slip = new_slip(&format!("追加 {index:03}"));
        let start = Instant::now();
        let outcome = store.apply(&[Mutation::Append(slip)]);
        push_micros(&mut samples, start.elapsed());
        assert!(
            outcome.is_applied(),
            "第 {index} 次 append 未提交：{:?}",
            outcome.result
        );
    }

    let (p50, p95, max) = report_latency(
        "[2] 单条追加：1000 条之上连续 100 次 apply(Append)",
        samples,
    );
    println!(
        "        门限：p50 < {:.0} ms，p95 < {:.0} ms，max < {:.0} ms",
        WRITE_P50_LIMIT_US / 1_000.0,
        WRITE_P95_LIMIT_US / 1_000.0,
        WRITE_MAX_LIMIT_US / 1_000.0
    );
    assert!(p50 < WRITE_P50_LIMIT_US, "append p50 {p50:.1} µs 超过门限");
    assert!(p95 < WRITE_P95_LIMIT_US, "append p95 {p95:.1} µs 超过门限");
    assert!(max < WRITE_MAX_LIMIT_US, "append max {max:.1} µs 超过门限");

    // 100 次追加必须全部真的落到文件里。
    assert_eq!(
        store.snapshot().slips.len(),
        1_100,
        "追加后条数不对，测量可能踩在失败路径上"
    );
    pass("单条追加满足门限");
}

// ---------------------------------------------------------------------------
// 3. 勾选延迟
// ---------------------------------------------------------------------------

/// 测量在已有 1000 条的文件上连续 100 次 `Patch SetDone` 的单次延迟。
///
/// 证明的是**"点一下勾选框"的往返延迟**。它和追加走同一条"整文件重写"事务，
/// 但额外要做 ID 查找与局部行替换；两者数字接近才说明修改路径没有额外的
/// O(n) 拷贝或重复渲染。为了每次都是真实写入，这里在全批里交替 true/false
/// （同一个方向连点两次会被判为 Noop，不写盘，测出来的就不是写延迟了）。
#[test]
#[ignore = "性能测量，需要 --ignored --nocapture"]
fn toggle_done_latency_on_1000_line_inbox() {
    let _guard = serial();
    banner();
    let dir = vault();
    let path = dir.path().join("inbox.md");
    write_inbox(&path, &seed_lines(1_000));

    let store = MarkdownStore::new(&path);
    let snapshot = store.snapshot();
    assert_eq!(snapshot.slips.len(), 1_000, "种子文件必须可解析");
    // 固定打第一条：ID 查找的成本与位置无关（线性扫描），这里只要稳定。
    let target = snapshot.slips[0].id;

    let mut samples = Vec::with_capacity(100);
    for index in 0..100 {
        // 偶数次设完成、奇数次取消完成：每次都真的改变字节。
        let done = index % 2 == 0;
        let start = Instant::now();
        let outcome = store.apply(&[Mutation::Patch {
            id: target,
            patch: Patch::SetDone {
                done,
                at: OffsetDateTime::now_utc(),
            },
        }]);
        push_micros(&mut samples, start.elapsed());
        assert!(
            outcome.is_applied(),
            "第 {index} 次 SetDone({done}) 未提交：{:?}",
            outcome.result
        );
    }

    let (p50, p95, max) = report_latency(
        "[3] 勾选：1000 条之上连续 100 次 apply(Patch SetDone)",
        samples,
    );
    println!(
        "        门限：p50 < {:.0} ms，p95 < {:.0} ms，max < {:.0} ms",
        WRITE_P50_LIMIT_US / 1_000.0,
        WRITE_P95_LIMIT_US / 1_000.0,
        WRITE_MAX_LIMIT_US / 1_000.0
    );
    assert!(p50 < WRITE_P50_LIMIT_US, "patch p50 {p50:.1} µs 超过门限");
    assert!(p95 < WRITE_P95_LIMIT_US, "patch p95 {p95:.1} µs 超过门限");
    assert!(max < WRITE_MAX_LIMIT_US, "patch max {max:.1} µs 超过门限");

    // 100 次切换要么改了 checkbox，要么改了 done_at；条数绝不能变。
    assert_eq!(store.snapshot().slips.len(), 1_000, "Patch 不应改变条数");
    let reloaded = store.reload();
    assert!(reloaded.is_applied(), "测量后文件必须仍然可解析");
    assert_eq!(reloaded.snapshot.slips.len(), 1_000);
    pass("勾选延迟满足门限");
}

// ---------------------------------------------------------------------------
// 4. 文件体积
// ---------------------------------------------------------------------------

/// 测量 10000 条时 `inbox.md` 的字节数与平均每条字节数。
///
/// 证明的是**"Markdown 真源不会悄悄膨胀"**。这个文件是用户会打开、同步、备份的
/// 普通文本，体积直接决定同步与备份成本，也决定上面两条延迟会被放大多少。
/// 平均每条字节数是一个格式健康度指标：字段被丢会明显掉下去，格式写胖会涨上去。
#[test]
#[ignore = "性能测量，需要 --ignored --nocapture"]
fn file_size_for_10000_records() {
    let _guard = serial();
    banner();
    let dir = vault();
    let path = dir.path().join("inbox.md");
    const COUNT: usize = 10_000;
    let written = write_inbox(&path, &seed_lines(COUNT));

    let store = MarkdownStore::new(&path);
    let snapshot = store.snapshot();
    assert_eq!(snapshot.slips.len(), COUNT, "种子文件必须解析出 {COUNT} 条");

    let actual = std::fs::metadata(&path)
        .expect("读取 inbox.md 元数据")
        .len();
    assert_eq!(actual, written, "磁盘字节与生成字节不一致");
    let per_record = actual as f64 / COUNT as f64;

    println!("[4] 文件体积：{COUNT} 条记录");
    println!(
        "  {:<7}{:>14}{:>12}{:>16}",
        "lines", "bytes", "KB", "bytes/record"
    );
    println!(
        "  {:<7}{:>14}{:>12.1}{:>16.1}",
        COUNT,
        actual,
        actual as f64 / 1024.0,
        per_record
    );
    println!(
        "        门限：总字节 < {:.1} KB，平均每条 {AVG_RECORD_MIN_BYTES:.0}–{AVG_RECORD_MAX_BYTES:.0} 字节",
        SIZE_LIMIT_BYTES as f64 / 1024.0
    );
    assert!(
        actual < SIZE_LIMIT_BYTES,
        "{COUNT} 条文件 {actual} 字节，超过门限 {SIZE_LIMIT_BYTES} 字节"
    );
    assert!(
        (AVG_RECORD_MIN_BYTES..=AVG_RECORD_MAX_BYTES).contains(&per_record),
        "平均每条 {per_record:.1} 字节，落在 [{AVG_RECORD_MIN_BYTES}, {AVG_RECORD_MAX_BYTES}] 之外"
    );
    pass("文件体积满足门限");
}
