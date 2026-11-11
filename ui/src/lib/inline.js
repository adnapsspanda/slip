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

/** 链接末尾常见的中西文标点不该算进 href。 */
const TRAILING = /[.,;:!?)\]}'"”，。；：！？、）》」』】]+$/;

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
      const href = raw.replace(TRAILING, "");
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
