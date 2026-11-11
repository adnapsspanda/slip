/**
 * 把正文切成可渲染的片段：`**粗体**`、`` `代码` ``、裸链接。
 *
 * 为什么手工切分而不是 `{@html}`：`inbox.md` 是用户可编辑的文件，把它的内容当 HTML
 * 插进 DOM 等于给这个文件一个执行入口。Svelte 默认转义插值，本模块产出的结构里
 * **每一段都只当作文本或已知标签**渲染，注入面保持为零。
 *
 * 不追求完整 Markdown：只认这三种行内标记。其余原样保留——多认一种语法就多一处
 * 与"纯文本真源"打架的地方。
 */

/** 链接只认 http/https，与后端 `open_url` 的校验保持一致。 */
const LINK = /https?:\/\/[^\s<>"'`]+/;

/** 三个分支：粗体、行内代码、裸链接。顺序即优先级。 */
const INLINE = /\*\*([^*\n]+)\*\*|`([^`\n]+)`|(https?:\/\/[^\s<>"'`]+)/g;

/**
 * 链接末尾不该算进 href 的标点。
 *
 * 写成**字符表**而不是一个长正则，是为了能逐字符断言（见 `tools/check-ui-logic.mjs`）：
 * 漏一个全角符号，用户体验就是「点进去 404」或「链接带个句号」。
 *
 * 覆盖三类：
 * - ASCII 标点：`. , ; : ! ? ) ] } ' "`
 * - 中文常用全角：`。 ， ； ： ！ ？ 、 ） 》 」 』 】`
 * - 中文引号与开括号（成对出现时只可能落在末尾的那一半，以及被整体贴进来的开括号）
 *   `“ ” ‘ ’`、`（ 《 【`
 */
export const TRAILING_PUNCTUATION =
  ".,;:!?)]}'\"”’。，；：！？、）》」』】“”‘’（《【";

/** 在字符类里安全的转义（`] ^ [ & - \` 这几个必须转义）。 */
const escapeForCharClass = (chars) => chars.replace(/[\\\]^[&-]/g, "\\$&");

const TRAILING = new RegExp(`[${escapeForCharClass(TRAILING_PUNCTUATION)}]+$`);

/**
 * 去掉链接尾部的标点。
 *
 * 抽成函数是为了能测：`tokenize` 的输出是 token 数组，直接断言太绕。
 *
 * @param {string} href
 * @returns {string}
 */
export function stripTrailingPunctuation(href) {
  return href.replace(TRAILING, "");
}

/**
 * @param {string} text
 * @returns {Array<{type: "text"|"bold"|"code"|"link", value: string, href?: string}>}
 */
export function tokenize(text) {
  if (!text) return [];
  const tokens = [];
  let cursor = 0;

  for (const match of text.matchAll(INLINE)) {
    const index = match.index ?? 0;
    if (index > cursor) {
      tokens.push({ type: "text", value: text.slice(cursor, index) });
    }
    if (match[1] !== undefined) {
      tokens.push({ type: "bold", value: match[1] });
    } else if (match[2] !== undefined) {
      tokens.push({ type: "code", value: match[2] });
    } else {
      const raw = match[3] ?? "";
      const href = stripTrailingPunctuation(raw);
      tokens.push({ type: "link", value: href, href });
      // 被剥掉的标点要还给文本，否则「见 https://a.com。」里的句号会消失。
      const tail = raw.slice(href.length);
      if (tail) tokens.push({ type: "text", value: tail });
    }
    cursor = index + match[0].length;
  }

  if (cursor < text.length) {
    tokens.push({ type: "text", value: text.slice(cursor) });
  }
  return tokens;
}

/** 正文里是否有可点击的东西——列表据此决定要不要提示。 */
export function hasLink(text) {
  return typeof text === "string" && LINK.test(text);
}

/**
 * 剥掉行首的列表标记，**与 `slip-domain/split.rs::strip_list_prefix` 逐条对应**。
 *
 * 为什么要在前端也有一份：捕获框的上限预检会**拦住提交**，而领域层是先剥前缀再量
 * 长度。两边口径不一致时，`- ` + 2000 字的合法输入会被前端误判成 2002 字而拒收
 * （实测过）。改任何一边，另一边必须跟着改——`tools/verify-hotkey.ps1` 里有一条
 * 端到端用例守着这个等式。
 *
 * 规则（与 Rust 一致）：
 * - 无序 `- ` `* ` `+ `；有序 1~3 位数字 + `. ` 或 `) `；分隔符后必须跟空格；
 * - checkbox `[ ] ` / `[x] ` / `[X] ` 只在列表标记**之后**才剥；
 * - 只剥一层。
 */
export function stripListPrefix(line) {
  const s = line.replace(/^\s+/, "");
  let rest = null;

  if (/^[-*+] /.test(s)) {
    rest = s.slice(2);
  } else {
    const ordered = s.match(/^\d{1,3}[.)] /);
    if (ordered) rest = s.slice(ordered[0].length);
  }

  if (rest === null) return s;
  for (const box of ["[ ] ", "[x] ", "[X] "]) {
    if (rest.startsWith(box)) return rest.slice(box.length);
  }
  return rest;
}

/**
 * 粘贴进来的多行文本要不要清洗列表前缀。
 *
 * 与 Rust 的 `strip_list_prefix` 同源（同一套正则），但**这里只剥前缀、不丢空行**：
 * 用户粘进输入框的是"待编辑的文本"，不是"已提交的草稿"，擅自删空行会让他以为丢了东西。
 * 真正的空行清洗在落盘时由领域层负责。
 *
 * 返回 `changed: false` 时调用方应完全放行默认粘贴行为（不能用 preventDefault 打断）。
 *
 * @param {string} raw
 * @returns {{text: string, changed: boolean}}
 */
export function cleanPastedText(raw) {
  if (typeof raw !== "string" || raw === "") {
    return { text: raw, changed: false };
  }
  const lines = raw.split(/\r\n|\r|\n/);
  let changed = false;
  const cleaned = lines.map((line) => {
    const next = stripListPrefix(line);
    if (next !== line) {
      changed = true;
    }
    return next;
  });
  return { text: cleaned.join("\n"), changed };
}
