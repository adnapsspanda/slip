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

/**
 * 行内编辑时按 Tab / Shift+Tab 要跳到哪一条。
 *
 * 返回 `null` 表示"没有相邻项可跳"——调用方此时应当把焦点还给当前行，
 * 而不是跳到一个不存在的下标上（那会让焦点掉到 body，键盘用户直接失去位置）。
 *
 * @param {number} index 当前编辑项下标
 * @param {number} total 列表长度
 * @param {boolean} shift 是否按住 Shift（反向）
 * @returns {number|null}
 */
export function tabTarget(index, total, shift) {
  if (
    !Number.isFinite(index) ||
    !Number.isFinite(total) ||
    total <= 0 ||
    index < 0
  ) {
    return null;
  }
  const target = shift ? index - 1 : index + 1;
  if (target < 0 || target >= total) {
    return null;
  }
  return target;
}

/**
 * 当前提示条（notice）上"那个按钮"对应什么动作。
 *
 * 抽成纯函数的原因：这条判定被两个入口共用（键盘 Alt+Z 与按钮点击），
 * 一旦两处判断不一致，就会出现"键盘按了没反应、点按钮却能用"的割裂。
 *
 * @param {{undo?: boolean}|null|undefined} notice
 * @returns {"undo"|"dismiss"|"none"}
 */
export function noticeAction(notice) {
  if (!notice) {
    return "none";
  }
  // 有撤销动作就执行它；纯信息提示没有按钮，此时快捷键的语义是"收起"。
  return notice.undo === true ? "undo" : "dismiss";
}

/**
 * 过滤输入框里按某个键该做什么。
 *
 * 抽成纯函数的原因和前面几个一样：这三条分支（退出 / 进列表 / 交给输入框）
 * 一旦搞混，表现是"按了没反应"或"过滤框被关掉、词也丢了"，很难靠点两下发现。
 *
 * @param {string} key
 * @returns {"exit"|"focusFirst"|"none"}
 */
export function filterKeyIntent(key) {
  if (key === "Escape") {
    return "exit";
  }
  // ↓ 或 Enter：词已经打好了，焦点交给列表第一条。Enter 也算——它是最自然的那一个。
  if (key === "ArrowDown" || key === "Enter") {
    return "focusFirst";
  }
  return "none";
}
