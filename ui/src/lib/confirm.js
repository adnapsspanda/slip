/**
 * 危险操作的"两步确认"判定。
 *
 * 抽成纯函数是为了能断言时间窗口的边界——这类逻辑靠手点根本验不准
 * （窗口 4 秒，人很难卡在第 3999 毫秒点第二下）。
 */

/** 两步确认的有效窗口（毫秒）。太短用户来不及，太长等于没拦。 */
export const CONFIRM_WINDOW_MS = 4000;

/**
 * 第一下点了之后，第二下该算什么。
 *
 * - 还没武装 → `"arm"`（记下时间，进入确认态）；
 * - 已武装且没超时 → `"execute"`（真的执行）；
 * - 已武装但超时 → **退回 `"arm"`**：过期状态不能"接着也算数"，
 *   否则用户几分钟后手滑点一下就直接执行了。
 *
 * @param {{armedAt: number|null, now: number, windowMs?: number}} state
 * @returns {"arm"|"execute"}
 */
export function confirmStep({ armedAt, now, windowMs = CONFIRM_WINDOW_MS }) {
  if (typeof armedAt !== "number") {
    return "arm";
  }
  const elapsed = now - armedAt;
  // 时钟回拨（elapsed < 0）按"已过期"处理：宁可让用户再点一次，也不能误执行。
  if (elapsed < 0 || elapsed > windowMs) {
    return "arm";
  }
  return "execute";
}

/**
 * 二次确认的**状态迁移**：给出"这一下点完之后，武装时间戳该是什么"。
 *
 * 与 `confirmStep` 的分工：那个只回答"算不算确认"，这个回答"状态怎么变"。
 * 界面（`App.svelte` 的 `confirmGate`）需要的正是后者——一行 `armed = next.armedAt`
 * 就能同时表达"武装 / 执行 / **超时自动还原**"，不必在每个调用点各写一遍定时器语义。
 *
 * `armedAt === null` 且 `execute === false` 以外的两种结果：
 * - `{armedAt: now, execute: false}`：刚武装（或超时后重新武装，时间戳取"现在"）；
 * - `{armedAt: null, execute: true}`：窗口内第二下，执行并解除武装。
 *
 * @param {number|null|undefined} armedAt
 * @param {number} now
 * @param {number} [windowMs]
 * @returns {{armedAt: number|null, execute: boolean}}
 */
export function confirmNext(armedAt, now, windowMs = CONFIRM_WINDOW_MS) {
  if (confirmStep({ armedAt, now, windowMs }) === "execute") {
    return { armedAt: null, execute: true };
  }
  // 超时不是"继续倒数"，而是从这一刻重新计时——过期状态绝不能被下一次误点兑现。
  return { armedAt: now, execute: false };
}
