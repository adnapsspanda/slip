/**
 * 收起窗口时的淡出（R119）。
 *
 * 边界说明：窗口的显示/隐藏由 **shell 侧**（Tauri 的 `dismiss_window`）决定，
 * 前端只能让**容器**在窗口真正收起之前淡出 0.12 秒。这是一个"内容先淡、窗口后收"
 * 的组合，不是原生窗口动画；真正的窗口过渡（DWM 淡入淡出）在 Windows 上由系统
 * 按用户设置决定，应用改不了，硬做只会得到双份动画。
 *
 * 所以这里只做两件事：
 * 1. 给容器加 0.12 秒的不透明度过渡；
 * 2. **`prefers-reduced-motion: reduce` 时延迟直接归零**——CSS 里把过渡时长压到
 *    0.01ms 还不够：那只是"没动画"，窗口仍会晚 120ms 才收到收起命令，
 *    对这类用户就是一次白等。让延迟本身为 0 才是"立即消失"。
 */

/** 淡出时长（毫秒）。改这里必须同步 `app.css` 里 `.paper--leaving` 的时长。 */
export const CLOSE_FADE_MS = 120;

/**
 * 收起窗口前要等多久。
 *
 * @param {boolean} reducedMotion 用户是否要求减少动态效果
 * @returns {number} 毫秒；0 表示立刻收起（不做任何等待）
 */
export function closeFadeDelay(reducedMotion) {
  return reducedMotion === true ? 0 : CLOSE_FADE_MS;
}
