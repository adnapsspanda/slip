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
