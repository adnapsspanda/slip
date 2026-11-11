/**
 * 捕获框的提交意图判定。
 *
 * 抽成纯函数不是为了好看：这几条规则（回车提交、Shift+Enter 换行、输入法组合期
 * 一律不提交）**没法靠肉眼回归**——它们只在特定输入法、特定按键组合下才出问题。
 * 抽出来之后 `tools/check-ui-logic.mjs` 能直接覆盖所有组合，不需要跑 GUI。
 *
 * 返回：
 * - `"composing"`：输入法正在选词，**任何提交键都不提交**（回车是用来确认候选词的）；
 * - `"newline"`：Shift+Enter 换行，交给 textarea 自己处理；
 * - `"submit"`：提交（Enter 与 Ctrl/Cmd+Enter 完全等价）；
 * - `"none"`：与本模块无关的按键。
 *
 * @param {{key?: string, shiftKey?: boolean, ctrlKey?: boolean, metaKey?: boolean, isComposing?: boolean, keyCode?: number}} event
 * @returns {"composing"|"newline"|"submit"|"none"}
 */
export function submitIntent(event) {
  if (!event || event.key !== "Enter") {
    return "none";
  }
  // 输入法组合期必须最先判：此时回车是"确认候选词"，提交会把半截拼音收进去。
  // `keyCode === 229` 是 WebView2 上部分输入法的兜底信号（`isComposing` 未必置位），
  // 中文用户最容易踩的就是这一条。
  if (event.isComposing === true || event.keyCode === 229) {
    return "composing";
  }
  // Shift 优先于 Ctrl：Shift+Enter 的语义是"换行"，加了 Ctrl 也还是换行，
  // 否则用户想换行却把内容提交了。
  if (event.shiftKey === true) {
    return "newline";
  }
  // Enter 与 Ctrl+Enter（macOS 上是 Cmd+Enter）完全等价。
  return "submit";
}

/**
 * 列表里的整页/首尾跳转。
 *
 * 为什么抽出来：这类索引计算最容易出边界错（越界、空列表、恰好落在末页），
 * 而靠肉眼点几遍是验不出来的。纯函数 + 断言就能把边界全覆盖。
 *
 * @param {number} index 当前焦点所在下标
 * @param {number} total 列表长度
 * @param {string} key 按键名
 * @returns {number|null} 目标下标；`null` 表示这个键不归本函数管
 */
export function listNavTarget(index, total, key) {
  if (!Number.isFinite(total) || total <= 0) {
    return null;
  }
  if (!Number.isFinite(index) || index < 0) {
    index = 0;
  }
  const last = total - 1;
  switch (key) {
    case "PageDown":
      // 一页 = 5 行；不足 5 行就落到末尾。
      return Math.min(index + 5, last);
    case "PageUp":
      return Math.max(index - 5, 0);
    case "Home":
      return 0;
    case "End":
      return last;
    default:
      return null;
  }
}
