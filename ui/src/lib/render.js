/**
 * 正文的**渲染视图**：把 token 流拼成可以 `#each` 出来的片段，顺带做两件事——
 * 中英混排的微间距（R87）与搜索高亮（R90）。
 *
 * 为什么单独一层：这两件事都只能在渲染层做。把它们写进 `slip.text` 会直接撞上
 * "未修改行逐字节保留"——正文是用户字节，派生视图一改，落盘、搜索、导出就全都
 * 看到被加工过的内容。所以本模块的输入是正文，输出是**片段**：正文本身只读。
 *
 * 一条可以机器检查的契约：所有非 `gap` 片段的 `value` 按顺序拼起来必须**逐字节
 * 等于**输入正文。`tools/check-ui-logic.mjs` 里对纯文本、带标记、带命中三种情况
 * 各断言一次。
 */
import { tokenize } from "./inline.js";
import { highlightSlices } from "./filter.js";

/**
 * 汉字（含扩展 A、兼容区）。刻意**不**收全角标点：`中文，English` 之间不加间距，
 * 标点自己带留白；收进来只会让每个逗号后面多出 0.2ch 的缝。
 */
const HAN = /[\u3400-\u4dbf\u4e00-\u9fff\uf900-\ufaff]/;

/** "ASCII 字母/数字"——混排间距只在这种字符与汉字相邻时才需要。 */
const ASCII_ALNUM = /[a-z0-9]/i;

/**
 * 这两个字符相邻时是否需要插一个微间距元素。
 *
 * 只处理"汉字 ↔ ASCII 字母/数字"这一种交界：汉字与汉字之间本来就有字面留白，
 * ASCII 与 ASCII 之间是单词内部，两者都不该动。
 *
 * @param {string} left 左邻字符（单个码位）
 * @param {string} right 右邻字符（单个码位）
 * @returns {boolean}
 */
export function needsGap(left, right) {
  if (!left || !right) return false;
  if (HAN.test(left) && ASCII_ALNUM.test(right)) return true;
  return ASCII_ALNUM.test(left) && HAN.test(right);
}

/** 取最后一个码位（`Array.from` 才不会把代理对切成半个字符）。 */
const lastChar = (value) => Array.from(value).pop() ?? "";
/** 取第一个码位。 */
const firstChar = (value) => Array.from(value)[0] ?? "";

/**
 * 把一段文字按混排交界的判定切成若干段，段与段之间就是要插间距的位置。
 *
 * 切分不产生新字符：输出各段拼接恒等于输入（断言守着这一条）。
 *
 * @param {string} value
 * @returns {string[]}
 */
export function splitMixRuns(value) {
  const runs = [];
  let current = "";
  let previous = "";
  for (const char of Array.from(value)) {
    if (current !== "" && needsGap(previous, char)) {
      runs.push(current);
      current = "";
    }
    current += char;
    previous = char;
  }
  if (current !== "") runs.push(current);
  return runs;
}

/**
 * 把一条纸条的正文拼成渲染片段。
 *
 * 片段类型：
 * - `text` / `bold` / `code` / `link`：与 `inline.js` 的 token 一致，`hit` 表示该段命中过滤词；
 * - `gap`：零宽度的微间距占位（`value` 恒为空串，渲染成空 `<span>`）。
 *
 * 交界判定跨 token 也成立：`**粗**abc` 里 `粗` 与 `a` 相邻时同样补间距——否则
 * "有没有间距"会取决于用户恰好用了哪种行内标记，这是没有道理的差别。
 *
 * @param {string} text 正文（只读）
 * @param {string | ReturnType<typeof import("./filter.js").parseQuery>} query 过滤条件
 * @returns {Array<{type: "text"|"bold"|"code"|"link"|"gap", value: string, hit: boolean, href?: string}>}
 */
export function buildInline(text, query) {
  const tokens = tokenize(typeof text === "string" ? text : "");
  const pieces = [];
  let previous = "";

  const push = (piece) => {
    if (needsGap(previous, firstChar(piece.value))) {
      pieces.push({ type: "gap", value: "", hit: false });
    }
    pieces.push(piece);
    previous = lastChar(piece.value);
  };

  for (const token of tokens) {
    if (token.type === "link") {
      const value = token.value ?? "";
      if (value !== "")
        push({ type: "link", value, hit: false, href: token.href });
      continue;
    }
    // 高亮：普通文本、粗体、行内代码都要能标出来。链接内部不标——
    // `<mark>` 嵌在 `<a>` 里会影响下划线与点击热区，而链接本身已经足够显眼。
    for (const slice of highlightSlices(token.value ?? "", query)) {
      for (const run of splitMixRuns(slice.value)) {
        push({ type: token.type, value: run, hit: slice.hit });
      }
    }
  }
  return pieces;
}
