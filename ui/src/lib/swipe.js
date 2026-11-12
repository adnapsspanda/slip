/**
 * 触控右滑完成（R114）的判定。
 *
 * 为什么抽成纯函数：阈值判定和"这一下算横滑还是竖滑"是最容易被手指的抖动搞错的地方
 * ——真机上验一次要拿触摸屏反复滑，而这里只要两个数字。
 *
 * 三条规矩：
 * 1. **只认右滑**：左滑不触发任何动作（左滑在很多应用里是"归档/删除"，这里没这功能，
 *    半跟随会让用户以为有）；
 * 2. **竖向优先**：`|dy| >= |dx|` 时判为滚动，交还给列表——否则斜着滑一下就误完成；
 * 3. **过阈值才算**：`SWIPE_COMPLETE_PX` 是手指位移，不是转速、不是时长。
 */

/** 右滑多少像素算"完成"。80px 在 13.5px 字号下约等于三行文字宽，够明确又不难滑到。 */
export const SWIPE_COMPLETE_PX = 80;

/**
 * 这一下横滑该做什么。
 *
 * @param {{dx?: number, dy?: number, threshold?: number}} gesture
 * @returns {"complete"|"none"}
 */
export function swipeIntent({ dx, dy, threshold = SWIPE_COMPLETE_PX } = {}) {
  if (!Number.isFinite(dx) || !Number.isFinite(dy)) return "none";
  // 竖向分量不输横向 → 用户想滚列表（`touch-action: pan-y` 会让浏览器接管滚动）。
  if (Math.abs(dy) >= Math.abs(dx)) return "none";
  return dx >= threshold ? "complete" : "none";
}

/**
 * 跟手位移：只跟右滑、并夹一个上限。
 *
 * 夹上限的理由：行宽有限，跟着手指跑出容器会露出底下的纸面，看着像布局坏了。
 * 左滑返回 0（不跟手），这样"左滑"干脆利落、不会给人"再滑一点就有事发生"的错觉。
 *
 * @param {number} dx
 * @param {number} [max]
 * @returns {number}
 */
export function swipeOffset(dx, max = 120) {
  if (!Number.isFinite(dx) || dx <= 0) return 0;
  return Math.min(dx, max);
}
