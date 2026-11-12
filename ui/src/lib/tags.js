/**
 * 从纸条正文里提取 `#标签` —— 与 `crates/slip-domain/src/tags.rs::extract_tags`
 * **逐条同规则**。
 *
 * 为什么要两份：Rust 那份定的是"标签是什么"（落盘与查询的语义），这份定的是
 * "标签在界面上长什么样"（徽章）。两份规则只要有一处走样，就会出现"看得见的
 * 标签过滤不到、过滤得到的标签看不见"。`tools/check-ui-logic.mjs` 用 Rust 测试里
 * 的同一批用例把这个等式钉住（URL 锚点、尾标点、去重、大小写）。
 *
 * 硬约束：**只读**。标签是正文的派生视图——不改写 `text`、不落盘、不进 marker。
 * 否则就破坏了"未修改行逐字节保留"，也让搜索与导出看到被加工过的内容。所以这里
 * 返回的是标签名数组，正文原样不动；徽章只是渲染层的另一段 DOM。
 *
 * 为什么用字符串表而不是 `Set` 字面量：这类表只做"某个单字符在不在里面"的判定，
 * `"…".includes(ch)` 已经够用，而 `new Set([...])` 在产物里要大得多——前端 JS 预算
 * 只有 88 KB，每处小表都值得省。
 */

/**
 * 可以作为标签结尾、应当被剥掉的标点（中英文常见的那几个），与 Rust 的
 * `is_trailing_punct` 一一对应。
 *
 * 注意这些字符**只在末尾**被剥：`#前#后` 那种中间带 `#` 的直接判定为不是标签。
 * 这里刻意**不含**弯引号 `” ’`：Rust 侧也只认直引号，多剥一个字符就是两套规则。
 */
const TRAILING_PUNCT = ",.;:!?)]}\"'。，、；：！？）》】」』…";

/** 标签前面可能出现的开括号：`（#工作）` 里 `#` 前面还有个全角括号。 */
const LEADING_WRAPPER = "([{（【《「『\"'";

/**
 * 从一个条目的正文里提取标签，按**首次出现顺序**去重。
 *
 * 规则（每条都对应 `tools/check-ui-logic.mjs` 里的断言）：
 * 1. 按空白切成 token——标签不能跨空格；
 * 2. token 去掉开头的括号后，必须以 `#` 打头；
 * 3. `#` 之后不能为空、不能再含 `#`、不能含路径分隔符；
 * 4. 末尾的中英文标点会被剥掉（`#工作，` → `工作`）。
 *
 * **URL 里的 `#anchor` 天然被排除**：`https://a.com#b` 这个 token 不以 `#` 开头，
 * 规则 2 就把它挡掉了。不需要额外判断"是不是 URL"——这一点和 Rust 侧完全一样。
 *
 * @param {string} text
 * @returns {string[]} 标签名（不含 `#`），大小写原样保留
 */
export function extractTags(text) {
  if (typeof text !== "string" || text === "") return [];
  const tags = [];
  // `\s+` 切分后首尾可能留下空串（Rust 的 split_whitespace 不会产出空 token）。
  for (const raw of text.split(/\s+/)) {
    if (!raw) continue;
    // 剥开头的括号/引号：`（#工作）` 里 `#` 前面还有个全角括号。
    let token = raw;
    while (LEADING_WRAPPER.includes(token[0])) token = token.slice(1);
    // 剥末尾标点。Rust 的 trim_end_matches 也会连剥多个，所以这里用 while。
    while (token && TRAILING_PUNCT.includes(token.slice(-1))) {
      token = token.slice(0, -1);
    }
    if (!token.startsWith("#")) continue;
    const rest = token.slice(1);
    // 空标签（只有 `#`）、多重 `#`（`#a#b`）、路径形式（`#a/b`）都不算。
    if (rest === "" || rest.includes("#")) continue;
    if (rest.includes("/") || rest.includes("\\")) continue;
    // 纯标点的"标签"没有意义（`#!!`）。与 Rust 的 `is_alphanumeric` 同口径：
    // 只看 Unicode 字母与数字，下划线/连字符不算"有内容"。
    if (!/[\p{L}\p{N}]/u.test(rest)) continue;
    // 去重但保留首次出现的顺序；名字大小写敏感（`#Work` 与 `#work` 是两个标签）。
    if (!tags.includes(rest)) tags.push(rest);
  }
  return tags;
}
