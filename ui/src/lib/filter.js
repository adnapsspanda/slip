/**
 * 过滤框的**纯判定**：`#tag`、`is:done` / `is:open` / `is:locked`，其余一律当全文模糊搜索。
 *
 * 为什么抽成纯函数：过滤是"只影响显示"的视图行为，但它有三条容易写歪的语义——
 * 标签精确匹配、状态关键字、以及不合法关键字的降级。写成纯函数才能在
 * `tools/check-ui-logic.mjs` 里一条一条钉住，不必启动窗口做 CDP 取证。
 *
 * 一条自我约束：这里**只读** `slip.text` / `slip.done` / `slip.locked`，
 * 不产生新的数据结构去落盘，也不改写任何字段。
 */
import { extractTags } from "./tags.js";

/**
 * 三个状态关键字。
 *
 * `is:open` 是"未完成"而不是"未归档"：列表与归档抽屉各自已经限定了范围
 * （`App.svelte` 的 `activeSlips` / `archivedSlips`），过滤只该回答"完成与否"。
 */
const STATES = {
  done: (slip) => !!slip.done,
  open: (slip) => !slip.done,
  locked: (slip) => !!slip.locked,
};

/** 状态关键字的合法名字，只为断言与文档服务（不参与判定）。 */
export const FILTER_STATES = Object.keys(STATES);

/**
 * 把过滤框里的原始输入解析成判定条件。
 *
 * 拆成 `{ text, tags, states }` 三组的好处：`highlightSlices` 只需要 `text`，
 * 而"是否真在做过滤"这一判断（空输入 vs 只写了不合法关键字）也有了唯一出处。
 *
 * @param {string} query
 * @returns {{text: string[], tags: string[], states: Array<(slip: any) => boolean>, degraded: boolean, constrained: boolean}}
 */
export function parseQuery(query) {
  const source = typeof query === "string" ? query.trim() : "";
  const out = {
    text: [],
    tags: [],
    states: [],
    degraded: false,
    constrained: false,
  };
  if (source === "") return out;

  for (const term of source.split(/\s+/)) {
    if (term === "") continue;
    const lower = term.toLowerCase();

    // `#xxx`：标签过滤。**借用 extractTags 做合法性判定**，而不是再抄一份规则——
    // 抄一份就会有两套"什么算标签"的判断，而它们迟早会分叉。
    if (term.startsWith("#")) {
      const [tag] = extractTags(term);
      if (tag) {
        out.tags.push(tag);
        continue;
      }
    }

    if (lower.startsWith("is:")) {
      const state = STATES[lower.slice(3)];
      if (state) {
        out.states.push(state);
        continue;
      }
      /*
       * 不认识的 `is:xxx` **优雅降级**：既不报错，也不把列表清空。
       *
       * 为什么选"忽略这个 term"而不是"把它当普通关键字搜"：后者会对
       * `is:bogus` 这样的输入匹配到 0 条——用户看到的就是一个空列表，
       * 和"这个关键字不支持"完全无法区分，而这正是要避免的失败形态。
       * 忽略之后，同一句里的其它关键字照常生效；真想搜字面量 `is:xxx`，
       * 去掉 `is:` 前缀当普通词搜即可。
       */
      out.degraded = true;
      continue;
    }

    // 普通关键字：大小写不敏感的子串匹配（与 R84 之前的行为一致）。
    out.text.push(lower);
  }

  out.constrained =
    out.text.length > 0 || out.tags.length > 0 || out.states.length > 0;
  return out;
}

/**
 * 这一条纸条是否应当出现在过滤结果里。
 *
 * 多个条件之间是 **AND**：`is:open #工作 报告` 的意思是"未完成、有 #工作、
 * 且正文里含『报告』"。短语按空白切开后逐词 AND，比原来的"整串子串匹配"
 * 更符合直觉，也不会让用户为了一个空格去猜。
 *
 * @param {{text?: string, done?: boolean, locked?: boolean} | null} slip
 * @param {string | ReturnType<typeof parseQuery>} query 原始输入或已解析的条件
 * @returns {boolean}
 */
export function matchesFilter(slip, query) {
  if (!slip) return false;
  const parsed = typeof query === "string" ? parseQuery(query) : query;
  if (!parsed || !parsed.constrained) return true;

  const text = typeof slip.text === "string" ? slip.text : "";
  const lower = text.toLowerCase();
  for (const term of parsed.text) {
    if (!lower.includes(term)) return false;
  }
  for (const tag of parsed.tags) {
    // 标签名**大小写敏感**（与 tags.js / Rust 的身份规则一致）：`#Work` 不等于 `#work`。
    // 普通关键字则是大小写不敏感的模糊搜索——两者的语义本来就不同。
    if (!extractTags(text).includes(tag)) return false;
  }
  for (const state of parsed.states) {
    if (!state(slip)) return false;
  }
  return true;
}

/**
 * 把一段正文按"命中/未命中"切成片段，供渲染层用 `#each` + `<mark>` 强调。
 *
 * 为什么返回片段而不是包一段 HTML 字符串：本项目零 HTML 注入（`{@html}`、
 * `innerHTML` 全禁），高亮只能靠"切文本 + 已知标签"实现。返回值拼起来必须
 * **逐字节等于原文**——这是不可回退的约束，断言里专门有一条守着。
 *
 * @param {string} text
 * @param {string | ReturnType<typeof parseQuery>} query
 * @returns {Array<{value: string, hit: boolean}>}
 */
export function highlightSlices(text, query) {
  const source = typeof text === "string" ? text : "";
  if (source === "") return [];
  const parsed = typeof query === "string" ? parseQuery(query) : query;
  const terms = parsed?.text ?? [];
  if (terms.length === 0) return [{ value: source, hit: false }];

  // 大小写不敏感地找全部出现位置（按小写副本定位，长度在 ASCII/CJK 下不变）。
  const lower = source.toLowerCase();
  const spans = [];
  for (const term of terms) {
    if (term === "") continue;
    let from = 0;
    for (;;) {
      const at = lower.indexOf(term, from);
      if (at < 0) break;
      spans.push([at, at + term.length]);
      from = at + term.length;
    }
  }
  if (spans.length === 0) return [{ value: source, hit: false }];

  // 多词命中可能重叠（`报告` 与 `报告书`）：合并成不重叠的区间再切片。
  spans.sort((a, b) => a[0] - b[0] || a[1] - b[1]);
  const merged = [];
  for (const span of spans) {
    const last = merged[merged.length - 1];
    if (last && span[0] <= last[1]) {
      last[1] = Math.max(last[1], span[1]);
    } else {
      merged.push([span[0], span[1]]);
    }
  }

  const slices = [];
  let cursor = 0;
  for (const [start, end] of merged) {
    if (start > cursor)
      slices.push({ value: source.slice(cursor, start), hit: false });
    slices.push({ value: source.slice(start, end), hit: true });
    cursor = end;
  }
  if (cursor < source.length)
    slices.push({ value: source.slice(cursor), hit: false });
  return slices;
}
