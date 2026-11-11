/**
 * 相对时间描述。纯函数，`now` 可注入——这样才测得了边界。
 *
 * **刻意不用定时器轮询**：纸条上的"10 分钟前"不需要每秒准确，为它挂着 setInterval
 * 只会让整个列表每秒重渲染一次。这里在渲染时按当前时刻算一次，快照更新或组件重新
 * 挂载时自然会再算。
 */

const MINUTE = 60;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

/**
 * @param {string} iso RFC3339 字符串
 * @param {number} [now] 毫秒时间戳，默认取当前时刻
 * @returns {string} 人话描述；无法解析时返回空串（界面据此不显示，而不是显示 "NaN"）
 */
export function relativeTime(iso, now = Date.now()) {
  const then = Date.parse(iso);
  if (Number.isNaN(then)) return "";

  const seconds = Math.floor((now - then) / 1000);
  // 时钟回拨（系统时间被改、或文件来自"未来"）不该显示成负数。
  if (seconds < MINUTE) return "刚刚";
  if (seconds < HOUR) return `${Math.floor(seconds / MINUTE)} 分钟前`;
  if (seconds < DAY) return `${Math.floor(seconds / HOUR)} 小时前`;

  const days = Math.floor(seconds / DAY);
  if (days === 1) return "昨天";
  if (days < 7) return `${days} 天前`;

  const d = new Date(then);
  const sameYear = d.getFullYear() === new Date(now).getFullYear();
  return sameYear
    ? `${d.getMonth() + 1} 月 ${d.getDate()} 日`
    : `${d.getFullYear()} 年 ${d.getMonth() + 1} 月 ${d.getDate()} 日`;
}

/** 悬浮提示用的精确本地时间。 */
export function exactLocal(iso) {
  const then = Date.parse(iso);
  if (Number.isNaN(then)) return "";
  return new Date(then).toLocaleString("zh-CN", { hour12: false });
}
