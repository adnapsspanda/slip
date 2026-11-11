//! 失焦自动收走的判定规则。
//!
//! 这段规则原本只写在 `src-tauri/src/hotkey.rs` 的一个 `window.on_window_event` 闭包里，
//! 判断依据是三个全局静态变量 + `Instant`——**没有任何办法单测**。而它出的问题
//! （窗口一闪就没、双击 exe 看起来没启动）恰恰是最难靠肉眼复现的那类。
//!
//! 所以把**规则**抽出来：纯函数、参数显式、不碰时间也不碰窗口。
//! 壳层负责把 `Instant` 换算成"距离上次唤出过了多久"，把静态变量读成参数。

use std::time::Duration;

/// 唤出后多久之内收到的"失焦"事件要忽略。
///
/// 原因：Windows 的前台锁定可能让 `set_focus()` 失败，紧接着系统就送来一个
/// `Focused(false)`；不设宽限期就会变成"窗口一闪就没了"。
pub const SUMMON_GRACE: Duration = Duration::from_millis(400);

/// 这次失焦事件要不要把窗口收走。
///
/// 三道闸门，顺序不能变：
///
/// 1. **没上膛**（启动展示）→ 永不自动收走。启动出来的窗口是普通窗口，
///    用户可能正在看它；别的程序抢前台不该把它收走。
/// 2. **从未聚焦过** → 不收。这不是"失去焦点"，而是"还没拿到焦点"；
///    收走等于让窗口闪现即消失——双击启动的经典症状。
/// 3. **还在宽限期内** → 不收。给前台锁定失败留一点缓冲。
///
/// 三者都不成立才收。
///
/// 参数刻意用"距离上次唤出的时长"而不是时间点：这样调用方用 `Instant` 也好、
/// 测试里直接给 `Duration` 也好，行为完全一致，且不存在时钟回拨的歧义。
pub fn should_dismiss(
    armed: bool,
    focused_since_summon: bool,
    elapsed_since_summon: Option<Duration>,
    grace: Duration,
) -> bool {
    if !armed {
        return false;
    }
    if !focused_since_summon {
        return false;
    }
    match elapsed_since_summon {
        // 从没唤出过却有失焦事件：保守起见按"在宽限期内"处理，不收。
        None => false,
        Some(elapsed) => elapsed >= grace,
    }
}

#[cfg(test)]
mod tests {
    use super::{SUMMON_GRACE, should_dismiss};
    use std::time::Duration;

    fn ms(value: u64) -> Option<Duration> {
        Some(Duration::from_millis(value))
    }

    #[test]
    fn never_dismisses_before_the_window_was_focused() {
        // 矩阵 R64 的核心要求：未曾聚焦前，绝不因失焦信号关闭。
        // 这正是"双击 exe → 前台锁定失败 → 系统立刻发 Focused(false)"的场景。
        assert!(!should_dismiss(true, false, ms(0), SUMMON_GRACE));
        assert!(!should_dismiss(true, false, ms(10_000), SUMMON_GRACE));
        assert!(!should_dismiss(true, false, None, SUMMON_GRACE));
    }

    #[test]
    fn ignores_blur_within_the_grace_window() {
        // 聚焦之后立刻来的失焦，仍要缓冲 400ms。
        assert!(!should_dismiss(true, true, ms(0), SUMMON_GRACE));
        assert!(!should_dismiss(true, true, ms(399), SUMMON_GRACE));
    }

    #[test]
    fn dismisses_after_grace_when_focused_then_left() {
        assert!(should_dismiss(true, true, ms(400), SUMMON_GRACE));
        assert!(should_dismiss(true, true, ms(5000), SUMMON_GRACE));
    }

    #[test]
    fn never_dismisses_when_not_armed() {
        // 启动展示不上膛：本机 Clash Verge 每约 2.7 秒抢一次前台，
        // 上膛的话窗口必然消失，用户以为没启动。
        assert!(!should_dismiss(false, true, ms(60_000), SUMMON_GRACE));
        assert!(!should_dismiss(false, false, ms(60_000), SUMMON_GRACE));
    }

    #[test]
    fn missing_summon_timestamp_is_treated_conservatively() {
        // 没有唤出记录却有失焦事件：宁可不收，也不能把窗口收掉。
        assert!(!should_dismiss(true, true, None, SUMMON_GRACE));
    }

    #[test]
    fn grace_boundary_is_inclusive_on_the_dismiss_side() {
        // 恰好等于宽限期就收：边界取"到了就收"，避免无限期拖着。
        assert!(should_dismiss(true, true, Some(SUMMON_GRACE), SUMMON_GRACE));
        assert!(!should_dismiss(
            true,
            true,
            Some(SUMMON_GRACE - Duration::from_millis(1)),
            SUMMON_GRACE
        ));
    }
}
