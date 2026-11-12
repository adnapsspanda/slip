#!/usr/bin/env bun
// 前端纯逻辑的检查（不需要启动应用、不需要浏览器）。
//
// 为什么单独有它：像 `bannerLevel` 这样的判定是**纯函数**，但要验它就得把 GUI 跑起来
// 做 CDP 取证——而本机 Smart App Control 时不时拒绝执行新编出的产物。把这些判定抽出来
// 之后，一条 `bun` 命令就能验，不受环境影响。
//
// 用法：& "C:\Users\spand\.bun\bin\bun.exe" run tools/check-ui-logic.mjs
import { readFile } from "node:fs/promises";
import {
  bannerLevel,
  shouldShowReadonlyGuide,
  hotkeyWarning,
  shouldSilentRefresh,
  SILENT_REFRESH_GAP_MS,
} from "../ui/src/lib/status.js";
import {
  createSerialQueue,
  nextToggleTarget,
  shouldDropPendingIntent,
} from "../ui/src/lib/queue.js";
import { confirmNext, confirmStep, CONFIRM_WINDOW_MS } from "../ui/src/lib/confirm.js";
import {
  cleanPastedText,
  stripTrailingPunctuation,
  TRAILING_PUNCTUATION,
  tokenize,
} from "../ui/src/lib/inline.js";
import {
  submitIntent,
  listNavTarget,
  tabTarget,
  noticeAction,
  filterKeyIntent,
  draftClearIntent,
} from "../ui/src/lib/keys.js";
import { extractTags } from "../ui/src/lib/tags.js";
import { FILTER_STATES, highlightSlices, matchesFilter, parseQuery } from "../ui/src/lib/filter.js";
import { buildInline, needsGap, splitMixRuns } from "../ui/src/lib/render.js";
import { orderSlips } from "../ui/src/lib/order.js";
import {
  UNDO_BURST_WINDOW_MS,
  dropUndoBurst,
  extendUndoBurst,
  undoSuffix,
  undoNotice,
} from "../ui/src/lib/notice.js";
import {
  captureFailureHint,
  capacitySummary,
  purgeNotice,
  restoreCoverWarning,
} from "../ui/src/lib/messages.js";
import { contrastRatio, parseHex, relativeLuminance } from "../ui/src/lib/color.js";
import { SWIPE_COMPLETE_PX, swipeIntent, swipeOffset } from "../ui/src/lib/swipe.js";
import { CLOSE_FADE_MS, closeFadeDelay } from "../ui/src/lib/motion.js";
import { vaultFilePath } from "../ui/src/lib/bridge.js";
import { exactLocal, parseIso, relativeTime } from "../ui/src/lib/time.js";

let failed = 0;

/**
 * 递归深比较。
 *
 * 为什么不用 `JSON.stringify(actual) === JSON.stringify(expected)`：两者都会把
 * `NaN` 序列化成 `null`，于是「期望 null、实际 NaN」这种断言会假绿。纯函数的
 * 返回值就以原始类型/数组/普通对象为主，自己比较比引一个深比较库便宜。
 */
function deepEqual(a, b) {
  if (Object.is(a, b)) return true;
  if (Array.isArray(a) !== Array.isArray(b)) return false;
  if (Array.isArray(a)) {
    return a.length === b.length && a.every((value, index) => deepEqual(value, b[index]));
  }
  if (a && b && typeof a === "object" && typeof b === "object") {
    const keys = Object.keys(a);
    if (keys.length !== Object.keys(b).length) return false;
    return keys.every((key) => Object.hasOwn(b, key) && deepEqual(a[key], b[key]));
  }
  return false;
}

const check = (label, actual, expected) => {
  const ok = deepEqual(actual, expected);
  if (!ok) failed += 1;
  console.log(`${ok ? "[PASS]" : "[FAIL]"} ${label}` + (ok ? "" : ` — 实际 ${JSON.stringify(actual)}，期望 ${JSON.stringify(expected)}`));
};

const healthy = {
  runtime: { state: "running" },
  recovery: { state: "normal" },
  health: { writable: true, warnings: [] },
  needsReload: false,
};

check("没有快照 → info", bannerLevel(null), "info");
check("健康 → info", bannerLevel(healthy), "info");
check("Fatal → error", bannerLevel({ ...healthy, runtime: { state: "fatal" } }), "error");
check("已隔离 → warn", bannerLevel({ ...healthy, recovery: { state: "isolated" } }), "warn");
check("不可写 → warn", bannerLevel({ ...healthy, health: { writable: false, warnings: [] } }), "warn");
check("需重载 → warn", bannerLevel({ ...healthy, needsReload: true }), "warn");
check(
  "Fatal 优先于不可写 → error",
  bannerLevel({ ...healthy, runtime: { state: "fatal" }, health: { writable: false, warnings: [] } }),
  "error",
);
check("缺字段也不炸 → info", bannerLevel({}), "info");

// ---- R42 捕获框提交意图：回车 / Ctrl+Enter / Shift+Enter / 输入法组合期 ----
check("Enter 提交", submitIntent({ key: "Enter" }), "submit");
check("Ctrl+Enter 与 Enter 等价", submitIntent({ key: "Enter", ctrlKey: true }), "submit");
check("Cmd+Enter 等价（macOS）", submitIntent({ key: "Enter", metaKey: true }), "submit");
check("Shift+Enter 换行", submitIntent({ key: "Enter", shiftKey: true }), "newline");
check("Shift+Ctrl+Enter 仍是换行", submitIntent({ key: "Enter", shiftKey: true, ctrlKey: true }), "newline");
check(
  "输入法组合期 Enter 不提交",
  submitIntent({ key: "Enter", isComposing: true }),
  "composing",
);
check(
  "输入法组合期 Ctrl+Enter 也不提交",
  submitIntent({ key: "Enter", isComposing: true, ctrlKey: true }),
  "composing",
);
check(
  "WebView2 兜底 keyCode 229 不提交",
  submitIntent({ key: "Enter", keyCode: 229 }),
  "composing",
);
check("输入法组合期 Shift+Enter 不换行也不提交", submitIntent({ key: "Enter", isComposing: true, shiftKey: true }), "composing");
check("无关按键返回 none", submitIntent({ key: "a" }), "none");
check("ArrowDown 与本模块无关", submitIntent({ key: "ArrowDown" }), "none");

// ---- R43 列表整页/首尾跳转 ----
check("PageDown 从 0 跳 5 行", listNavTarget(0, 20, "PageDown"), 5);
check("PageDown 接近末尾时夹到末项", listNavTarget(18, 20, "PageDown"), 19);
check("PageDown 在末项不动", listNavTarget(19, 20, "PageDown"), 19);
check("PageUp 从 6 退 5 行", listNavTarget(6, 20, "PageUp"), 1);
check("PageUp 接近开头时夹到首项", listNavTarget(2, 20, "PageUp"), 0);
check("Home 到首项", listNavTarget(15, 20, "Home"), 0);
check("End 到末项", listNavTarget(3, 20, "End"), 19);
check("空列表不给目标", listNavTarget(0, 0, "PageDown"), null);
check("未知键不给目标", listNavTarget(3, 20, "Tab"), null);
check("非法下标按首项处理", listNavTarget(-7, 20, "PageDown"), 5);
check("单项列表 PageDown 停在 0", listNavTarget(0, 1, "PageDown"), 0);

// ---- R44 编辑态 Tab / Shift+Tab ----
check("Tab 跳到下一条", tabTarget(2, 5, false), 3);
check("Shift+Tab 跳到上一条", tabTarget(2, 5, true), 1);
check("末条 Tab 不跳（返回 null）", tabTarget(4, 5, false), null);
check("首条 Shift+Tab 不跳（返回 null）", tabTarget(0, 5, true), null);
check("单项列表 Tab 不跳", tabTarget(0, 1, false), null);
check("空列表不跳", tabTarget(0, 0, false), null);
check("非法下标不跳", tabTarget(-3, 5, false), null);

// ---- R45 粘贴时清洗列表前缀 ----
check("普通文本不变", cleanPastedText("买牛奶"), { text: "买牛奶", changed: false });
check("剥掉无序列表前缀", cleanPastedText("- 买牛奶"), { text: "买牛奶", changed: true });
check("剥掉星号前缀", cleanPastedText("* 买牛奶"), { text: "买牛奶", changed: true });
check("剥掉有序列表前缀", cleanPastedText("1. 买牛奶"), { text: "买牛奶", changed: true });
check("剥掉 - [ ] 复选框", cleanPastedText("- [ ] 买牛奶"), { text: "买牛奶", changed: true });
check("剥掉 - [x] 已完成框", cleanPastedText("- [x] 买牛奶"), { text: "买牛奶", changed: true });
check(
  "多行逐行剥并保留行数",
  cleanPastedText("- 买牛奶\n- 买面包"),
  { text: "买牛奶\n买面包", changed: true },
);
check("只剥第一层前缀，不重复剥", cleanPastedText("- - 买牛奶"), { text: "- 买牛奶", changed: true });
check("空字符串不动", cleanPastedText(""), { text: "", changed: false });

// ---- R47 提示条动作（按钮与键盘共用同一判定）----
check("无提示条 → 不动作", noticeAction(null), "none");
check("可撤销的提示条 → 撤销", noticeAction({ undo: true }), "undo");
check("纯信息提示 → 收起", noticeAction({ undo: false }), "dismiss");
check("缺字段按纯信息处理", noticeAction({}), "dismiss");

// ---- R48 链接尾部标点：矩阵点名的全角符号逐个验 ----
const CN_PUNCT = "。，！？；：“”‘’（）《》";
for (const ch of CN_PUNCT) {
  check(
    `尾部全角「${ch}」不算进链接`,
    stripTrailingPunctuation(`https://a.com${ch}`),
    "https://a.com",
  );
}
// 表里每一个字符都必须真的被剥掉（防止表里混进无效字符）
const notStripped = [...TRAILING_PUNCTUATION].filter(
  (ch) => stripTrailingPunctuation(`https://a.com${ch}`) !== "https://a.com",
);
check("字符表里没有失效的标点", notStripped, []);
check("多个尾部标点一起剥", stripTrailingPunctuation("https://a.com。）。"), "https://a.com");
check("干净链接不变", stripTrailingPunctuation("https://a.com/path?q=1"), "https://a.com/path?q=1");
check("中间标点保留", stripTrailingPunctuation("https://a.com/a,b"), "https://a.com/a,b");

// 端到端：tokenize 里被剥掉的标点要还给文本，不能吞字
const toks = tokenize("见 https://a.com。");
const linkTok = toks.find((t) => t.type === "link");
check("tokenize 的 href 不含句号", linkTok?.href, "https://a.com");
check(
  "tokenize 把句号还给文本",
  toks.map((t) => (t.type === "link" ? t.value : t.value)).join(""),
  "见 https://a.com。",
);

// ---- R49 快速连续勾选：意图反转 + 串行时序 ----
check("无意图时按快照反转", nextToggleTarget(false, undefined), true);
check("无意图时按快照反转（已完成）", nextToggleTarget(true, undefined), false);
check("有未落盘意图时以意图为准（连点第二下）", nextToggleTarget(false, true), false);
check("有未落盘意图时以意图为准（反向）", nextToggleTarget(true, false), true);
// 快照始终停在 false（还没来得及刷新），连续三次点击的目标应当是 true → false → true：
// 第 1 下按快照反转，第 2、3 下按"上一次的意图"反转。
const t1 = nextToggleTarget(false, undefined);
const t2 = nextToggleTarget(false, t1);
const t3 = nextToggleTarget(false, t2);
check("连续三次点击：true → false → true（净效果切回原状态）", [t1, t2, t3], [true, false, true]);

// 意图不是永久事实：失败、快照追上、或者换了一次会话时都必须作废。
// 留着一条"没发生过的意图"，下一回点击就会从它反转——症状是勾选要按两下才生效。
check("后端未接受、无排队 → 意图必须删除", shouldDropPendingIntent(0, false, false, true), true);
check("快照已等于目标 → 意图已落盘，删除", shouldDropPendingIntent(0, true, true, true), true);
check("快照还没追上 → 保留意图给下一次反转", shouldDropPendingIntent(0, true, false, true), false);
// 失败但快照碰巧等于目标时，仍然按"未接受"删除：快照相等不能证明是这次请求的功劳。
check("未接受优先于快照相等", shouldDropPendingIntent(0, false, true, true), true);
// 同一 id 还有排队点击时，哪怕这一条失败也不能删：删了第三次点击会从旧快照反转。
check("仍有排队请求 → 无论如何保留意图", shouldDropPendingIntent(1, false, true, true), false);

// 串行队列：任务必须按提交顺序执行，且一次只跑一个
const order = [];
let running = 0;
let maxConcurrent = 0;
const q = createSerialQueue();
const delay = (ms) => new Promise((r) => setTimeout(r, ms));
const tasks = [30, 5, 1].map((ms, i) =>
  q.run(async () => {
    running += 1;
    maxConcurrent = Math.max(maxConcurrent, running);
    await delay(ms);
    order.push(i);
    running -= 1;
    return i;
  }),
);
const results = await Promise.all(tasks);
check("串行队列按提交顺序执行", order, [0, 1, 2]);
check("串行队列同时只有一个任务在跑", maxConcurrent, 1);
check("串行队列返回各自结果", results, [0, 1, 2]);
check("队列跑完后计数归零", q.size(), 0);

// 一个任务失败不能让后续任务卡住
const q2 = createSerialQueue();
const ok1 = q2.run(async () => "ok");
const bad = q2.run(async () => {
  throw new Error("boom");
});
const ok2 = q2.run(async () => "after");
check("失败任务之后的排队任务仍会执行", await ok2, "after");
await bad.catch(() => {});
check("失败任务之前的结果不受影响", await ok1, "ok");

// ---- R50 过滤框：↓ / Enter 进首项，Esc 退出 ----
check("过滤框 ↓ 进首项", filterKeyIntent("ArrowDown"), "focusFirst");
check("过滤框 Enter 进首项", filterKeyIntent("Enter"), "focusFirst");
check("过滤框 Esc 退出", filterKeyIntent("Escape"), "exit");
check("普通字符交给输入框", filterKeyIntent("a"), "none");
check("过滤框 ↑ 不劫持", filterKeyIntent("ArrowUp"), "none");

// ---- R52 危险操作两步确认 ----
check("未武装时第一下只武装", confirmStep({ armedAt: null, now: 1000 }), "arm");
check("窗口内第二下执行", confirmStep({ armedAt: 1000, now: 2000 }), "execute");
check("恰好等于窗口仍执行", confirmStep({ armedAt: 1000, now: 1000 + CONFIRM_WINDOW_MS }), "execute");
check("超窗后重新武装（不会误执行）", confirmStep({ armedAt: 1000, now: 1000 + CONFIRM_WINDOW_MS + 1 }), "arm");
check("时钟回拨按过期处理", confirmStep({ armedAt: 5000, now: 1000 }), "arm");
check("自定义更短窗口", confirmStep({ armedAt: 0, now: 900, windowMs: 500 }), "arm");

// ---- R54 只读排查指引的显示条件 ----
check("可写时不显示指引", shouldShowReadonlyGuide({ health: { writable: true } }), false);
check("不可写时显示指引", shouldShowReadonlyGuide({ health: { writable: false } }), true);
check("缺字段时不显示", shouldShowReadonlyGuide({}), false);
check("没有快照时不显示", shouldShowReadonlyGuide(null), false);

// ---- R68 热键未注册的提示 ----
check("热键正常时无提示", hotkeyWarning({ registered: true, text: "Ctrl+Alt+Space" }), "");
check("没有状态时无提示", hotkeyWarning(null), "");
check(
  "热键未注册时提示里说清「未注册」",
  hotkeyWarning({ registered: false, text: "" }).includes("未注册"),
  true,
);
const hkWarn = hotkeyWarning({ registered: false, text: "Ctrl+Alt+Space" });
check("提示带上具体组合键", hkWarn.includes("Ctrl+Alt+Space"), true);
check("提示给出可操作下一步（托盘）", hkWarn.includes("托盘"), true);
check("提示说明原因（被占用）", hkWarn.includes("占用"), true);

// ---- R69 回到前台时的静默刷新限流 ----
check("从未刷过 → 要刷", shouldSilentRefresh(null, 10000), true);
check("刚刷过 → 不刷", shouldSilentRefresh(10000, 10000 + 100), false);
check("刚好到间隔 → 要刷", shouldSilentRefresh(10000, 10000 + SILENT_REFRESH_GAP_MS), true);
check("超过间隔 → 要刷", shouldSilentRefresh(10000, 60000), true);
check("时钟回拨 → 保守要刷", shouldSilentRefresh(10000, 5000), true);
check("非法值 → 要刷", shouldSilentRefresh(Number.NaN, 10000), true);

// ---- R84 `#标签` 提取：与 crates/slip-domain/src/tags.rs 的用例一一对应 ----
// 前端这份与 Rust 那份必须**规则完全一致**，否则界面上看得见的标签和能过滤到的
// 标签是两套东西。下面每条都摘自 Rust 侧的测试，改一边就得改另一边。
check("简单英文标签", extractTags("#work #life"), ["work", "life"]);
check("句中的标签", extractTags("今天要写 #work 的报告"), ["work"]);
check("中文标签", extractTags("做点事 #工作 #生活"), ["工作", "生活"]);
check("中英混合的标签名", extractTags("#待办2"), ["待办2"]);
check("URL 锚点不算标签", extractTags("https://a.com#b"), []);
check("句子里的 URL 锚点也不算法", extractTags("见 https://example.com/page#section 这里有说明"), []);
check("同一句里的真标签仍要认出来", extractTags("https://a.com#b 另外 #work"), ["work"]);
check("下划线在标签名里合法", extractTags("#work_name"), ["work_name"]);
check("纯符号的标签不算", extractTags("#-"), []);
check("括号包裹的中文标签", extractTags("（#工作）"), ["工作"]);
check("括号包裹的英文标签", extractTags("(#work)"), ["work"]);
check("直角引号包裹的标签", extractTags("「#工作」"), ["工作"]);
// 尾标点表逐个验（与 Rust 的 is_trailing_punct 同表）
const TAG_TRAILING = [",", ".", ";", ":", "!", "?", ")", "]", "}", '"', "'", "。", "，", "、", "；", "：", "！", "？", "）", "】", "》", "」", "』", "…"];
const tagNotStripped = TAG_TRAILING.filter((ch) => extractTags(`#work${ch}`).join() !== "work");
check("尾标点全部被剥掉", tagNotStripped, []);
check("多个尾标点一起剥", extractTags("#work!?"), ["work"]);
check("中文句号被剥掉", extractTags("#工作。"), ["工作"]);
// 退化 token：逐条都不该产出标签（与 Rust 的 rejects_degenerate_tokens 同集）
const DEGENERATE = ["#", "# ", "#a#b", "#a/b", "#!!", "没有标签", "a#b", "# 标题"];
check("退化 token 都不算标签", DEGENERATE.map((text) => extractTags(text).join("|")), DEGENERATE.map(() => ""));
check("按首次出现顺序去重", extractTags("#b #a #b #c #a"), ["b", "a", "c"]);
check("标签名大小写敏感", extractTags("#Work #work"), ["Work", "work"]);
const tagSource = "#工作 今天要把报告写完";
const tagSourceBefore = tagSource;
extractTags(tagSource);
check("提取标签不改写正文", tagSource, tagSourceBefore);

// ---- R85/R86 过滤：#tag 与 is:done / is:open / is:locked ----
const open = { text: "写 #工作 的报告", done: false, locked: false };
const done = { text: "买牛奶", done: true, locked: false };
const locked = { text: "已锁的 #工作 条目", done: false, locked: true };
check("空输入全部显示", matchesFilter(open, ""), true);
check("纯空白全部显示", matchesFilter(open, "   "), true);
check("全文模糊搜索（大小写不敏感）", matchesFilter({ text: "Buy Milk" }, "milk"), true);
check("全文模糊搜索未命中", matchesFilter({ text: "买牛奶" }, "面包"), false);
check("#tag 命中", matchesFilter(open, "#工作"), true);
check("#tag 未命中", matchesFilter(open, "#生活"), false);
check("标签大小写敏感（与领域层一致）", matchesFilter({ text: "写 #Work" }, "#work"), false);
check("过滤框里标签的尾标点也被剥", parseQuery("#工作，").tags, ["工作"]);
check("单独的 # 不是标签（当普通词搜）", parseQuery("#").tags, []);
check("is:done 命中已完成", matchesFilter(done, "is:done"), true);
check("is:done 不命中未完成", matchesFilter(open, "is:done"), false);
check("is:open 命中未完成", matchesFilter(open, "is:open"), true);
check("is:open 不命中已完成", matchesFilter(done, "is:open"), false);
check("is:locked 只看锁定标志", [matchesFilter(locked, "is:locked"), matchesFilter(open, "is:locked")], [true, false]);
check("条件之间是 AND", [matchesFilter(open, "is:open #工作"), matchesFilter(done, "is:open #工作")], [true, false]);
check("关键字大小写不敏感", matchesFilter(done, "IS:DONE"), true);
check("状态关键字表只有三个", FILTER_STATES, ["done", "open", "locked"]);
check("多词按空白切开后逐词 AND", [matchesFilter({ text: "买牛奶" }, "买 牛奶"), matchesFilter({ text: "买面包" }, "买 牛奶")], [true, false]);
// 不合法 is:xxx：不报错、也不清空列表；同一句里的其它条件照常生效
check("不合法 is:xxx 不构成约束", matchesFilter(open, "is:bogus"), true);
check("不合法 is:xxx 之后其它词仍生效", [matchesFilter(open, "is:bogus 报告"), matchesFilter(done, "is:bogus 报告")], [true, false]);
check("解析结果标记了降级", parseQuery("is:bogus").degraded, true);
check("空查询不算受约束", parseQuery("   ").constrained, false);
check("只有不合法 is: 时不算受约束", parseQuery("is:bogus").constrained, false);

// ---- R90 高亮切片：用文本切片而不是 HTML 注入 ----
check("命中处切成 hit 片段", highlightSlices("买入牛奶", "牛奶"), [
  { value: "买入", hit: false },
  { value: "牛奶", hit: true },
]);
check("没有命中时是单个非 hit 段", highlightSlices("买入牛奶", "面包"), [{ value: "买入牛奶", hit: false }]);
check("高亮大小写不敏感", highlightSlices("Buy Milk", "milk"), [
  { value: "Buy ", hit: false },
  { value: "Milk", hit: true },
]);
check("多处命中都标出来", highlightSlices("牛奶和牛奶", "牛奶").filter((s) => s.hit).map((s) => s.value), ["牛奶", "牛奶"]);
check(
  "重叠的多词命中合并成一段",
  highlightSlices("报告书", "报告 报告书"),
  [{ value: "报告书", hit: true }],
);
check("只有状态条件时不高亮正文", highlightSlices("买入牛奶", "is:done"), [{ value: "买入牛奶", hit: false }]);
// 高亮切片拼回去必须逐字节等于原文（不可回退：正文只读）
const HL_SOURCES = ["买入牛奶", "报告书 与 报告", "", "纯文本没有命中"];
const hlRebuilt = HL_SOURCES.filter((text) => highlightSlices(text, "报告 牛奶").map((s) => s.value).join("") !== text);
check("高亮切片拼回后与原文逐字节相同", hlRebuilt, []);

// ---- R87 中英混排微间距：只插零宽元素，不动正文字符 ----
check("汉字后紧跟字母要间距", needsGap("看", "a"), true);
check("字母后紧跟汉字要间距", needsGap("a", "看"), true);
check("汉字紧跟数字要间距", needsGap("第", "2"), true);
check("汉字之间不要间距", needsGap("看", "书"), false);
check("字母与数字之间不要间距", needsGap("a", "2"), false);
check("汉字与标点之间不要间距", needsGap("看", "，"), false);
check("混排切分保留全部字符", splitMixRuns("第2章abc").join(""), "第2章abc");
check("混排切分的段落", splitMixRuns("第2章abc"), ["第", "2", "章", "abc"]);
check("纯中文不切", splitMixRuns("买牛奶"), ["买牛奶"]);
const gapPieces = buildInline("看abc", "");
check("间距片段是零宽的（value 为空串）", gapPieces.filter((p) => p.type === "gap").map((p) => p.value), [""]);
check("间距片段个数与交界数一致", gapPieces.filter((p) => p.type === "gap").length, 1);
// 渲染视图的硬契约：非 gap 片段拼起来必须与 tokenize 的可见文本逐字节相同。
// 注：`**粗体**` 这类标记字符**本来就会被 tokenize 消费掉**（那是行内标记，不是正文），
// 所以基准取 tokenize 的拼接结果，而不是原始字符串。
const INLINE_SOURCES = [
  "看abc https://a.com#b 另外 **粗体**abc 和 `code`",
  "#工作 把 报告 写完",
  "见 https://a.com。",
  "",
];
const rebuilt = INLINE_SOURCES.filter((text) => {
  const expected = tokenize(text).map((token) => token.value).join("");
  const actual = buildInline(text, "报告 abc")
    .filter((piece) => piece.type !== "gap")
    .map((piece) => piece.value)
    .join("");
  return actual !== expected;
});
check("渲染片段拼回后与可见正文逐字节相同", rebuilt, []);
check(
  "过滤命中在渲染片段里被标出",
  buildInline("买入牛奶", "牛奶").filter((p) => p.hit).map((p) => p.value),
  ["牛奶"],
);
check(
  "没有过滤词时没有任何 hit 片段",
  buildInline("买入牛奶", "").filter((p) => p.hit).length,
  0,
);

// ---- R94 置顶排序：置顶稳定浮到最前，组内顺序不变 ----
const ids = (list) => list.map((item) => item.id);
const mk = (id, pinned = false) => ({ id, pinned });
check(
  "置顶浮到最前，其余保持原序",
  ids(orderSlips([mk("a"), mk("b", true), mk("c"), mk("d", true)])),
  ["b", "d", "a", "c"],
);
check("没有置顶时顺序不变", ids(orderSlips([mk("a"), mk("b")])), ["a", "b"]);
check("全部置顶时顺序不变", ids(orderSlips([mk("a", true), mk("b", true)])), ["a", "b"]);
check("只置顶最后一条时它排到最前", ids(orderSlips([mk("a"), mk("b"), mk("c", true)])), ["c", "a", "b"]);
check(
  "置顶组内部也保持原序（不是按 id 或时间重排）",
  ids(orderSlips([mk("z", true), mk("a"), mk("y", true)])),
  ["z", "y", "a"],
);
check("缺 pinned 字段当未置顶", ids(orderSlips([{ id: "a" }, { id: "b", pinned: true }])), ["b", "a"]);
check("空表与非数组都不炸", [orderSlips([]), orderSlips(null)], [[], []]);
const unsortedInput = [mk("a"), mk("b", true), mk("c")];
orderSlips(unsortedInput);
check("排序不改写入参", ids(unsortedInput), ["a", "b", "c"]);

// ---- R100 撤销连击：3 秒内连续操作要说清"还有几步可撤销" ----
const T0 = 1_000_000;
const burst1 = extendUndoBurst(null, T0);
check("第一次操作 → 1 步", burst1, { count: 1, at: T0 });
const burst2 = extendUndoBurst(burst1, T0 + 500);
const burst3 = extendUndoBurst(burst2, T0 + 1000);
check("3 秒内连续三次 → 计数 1、2、3", [burst1.count, burst2.count, burst3.count], [1, 2, 3]);
check("恰好等于窗口仍算同一串", extendUndoBurst({ count: 2, at: T0 }, T0 + UNDO_BURST_WINDOW_MS).count, 3);
check("超过窗口重新从 1 开始", extendUndoBurst({ count: 2, at: T0 }, T0 + UNDO_BURST_WINDOW_MS + 1).count, 1);
check("时钟回拨按新一轮处理", extendUndoBurst({ count: 5, at: T0 + 5000 }, T0).count, 1);
check("非法计数不传播", [extendUndoBurst({ count: 0, at: T0 }, T0).count, extendUndoBurst("x", T0).count], [1, 1]);
check("撤销一步 → 计数减一", dropUndoBurst({ count: 3, at: T0 }, T0 + 10), { count: 2, at: T0 + 10 });
check("减到 1 再撤销就清空", dropUndoBurst({ count: 1, at: T0 }, T0), null);
check("本来就没有计数时撤销仍是空", [dropUndoBurst(null, T0), dropUndoBurst({ count: 0, at: T0 }, T0)], [null, null]);
check("只有一步时不啰嗦", undoSuffix(1), "");
check("多步时说明还剩几步", undoSuffix(3), "，还有 3 步可撤销");
check("没有步骤时不加后缀", [undoSuffix(0), undoSuffix(Number.NaN)], ["", ""]);
check("通知文案带上剩余步数", undoNotice("已完成", 3), {
  message: "已完成，还有 3 步可撤销",
  undo: true,
});
check("单步通知只有原句", undoNotice("已完成", 1), { message: "已完成", undo: true });
// 一串三步的完整叙事：操作 → 撤销 → 再撤销 → 再撤销（最后一步之后没有撤销入口）
const trailA = extendUndoBurst(null, T0);
const trailB = extendUndoBurst(trailA, T0 + 400);
const trailC = dropUndoBurst(trailB, T0 + 800);
const trailD = dropUndoBurst(trailC, T0 + 1200);
const trailE = dropUndoBurst(trailD, T0 + 1600);
check("两步之后逐次回退直到没有可撤销", [trailC.count, trailE, undoSuffix(trailC.count)], [1, null, ""]);

// ---- R95/R98 提示条动作与清空草稿的组合键 ----
check("提示条带恢复草稿 → restore", noticeAction({ restore: "草稿原文" }), "restore");
check("同时带撤销与恢复时撤销优先", noticeAction({ undo: true, restore: "草稿原文" }), "undo");
check("纯提示 → 收起", noticeAction({ message: "已收录" }), "dismiss");
check("Ctrl+Backspace 清空草稿", draftClearIntent({ key: "Backspace", ctrlKey: true }), true);
check("Cmd+Backspace 等价（macOS）", draftClearIntent({ key: "Backspace", metaKey: true }), true);
check("裸 Backspace 删一个字，不清草稿", draftClearIntent({ key: "Backspace" }), false);
check("Ctrl+Shift+Backspace 是删整行", draftClearIntent({ key: "Backspace", ctrlKey: true, shiftKey: true }), false);
check("Alt+Backspace 是删整词", draftClearIntent({ key: "Backspace", ctrlKey: true, altKey: true }), false);
check("输入法组合期不清草稿", draftClearIntent({ key: "Backspace", ctrlKey: true, isComposing: true }), false);
check("WebView2 兜底 229 不清草稿", draftClearIntent({ key: "Backspace", ctrlKey: true, keyCode: 229 }), false);
check("别的键（Ctrl+A）不动草稿", draftClearIntent({ key: "a", ctrlKey: true }), false);

// ---- R107 磁盘被占用：必须说清"内容还在输入框里" ----
// 本项目不做任何本地暂存（那会造出第二真源），所以写入失败时**草稿仍在捕获框里**；
// 这条文案就是把这件事讲出来，否则用户会把"写不进去"理解成"内容没了"。
const ioHint = captureFailureHint({ result: "rejected", error: { code: "io", message: "文件被占用" } });
check("io 失败要提磁盘被占用", ioHint.includes("磁盘被占用"), true);
check("io 失败要说清内容还在输入框里", ioHint.includes("输入框"), true);
check("io 失败带上后端细节", ioHint.includes("文件被占用"), true);
check(
  "busy 失败同样提示草稿还在",
  captureFailureHint({ result: "rejected", error: { code: "busy" } }).includes("输入框"),
  true,
);
check(
  "其它错误不抢话（交给 formatAckError）",
  captureFailureHint({ result: "rejected", error: { code: "conflict" } }),
  "",
);
check("没有 ack 时不崩", [captureFailureHint(null), captureFailureHint(undefined)], ["", ""]);

// ---- R109 恢复备份的覆盖提示：报真实数字，拿不到就说"无法统计" ----
check("有 N 条未完成就报 N", restoreCoverWarning(3), "恢复后将覆盖当前 3 条未完成项");
check("一条也要报", restoreCoverWarning(1), "恢复后将覆盖当前 1 条未完成项");
check("零条如实说零", restoreCoverWarning(0), "恢复后将覆盖当前 0 条未完成项");
check(
  "拿不到数字时明说无法统计（不编 0）",
  [restoreCoverWarning(null), restoreCoverWarning(undefined), restoreCoverWarning(Number.NaN)],
  [
    "恢复后将覆盖当前内容（无法统计未完成条数）",
    "恢复后将覆盖当前内容（无法统计未完成条数）",
    "恢复后将覆盖当前内容（无法统计未完成条数）",
  ],
);
check(
  "负数按脏数据处理",
  restoreCoverWarning(-1),
  "恢复后将覆盖当前内容（无法统计未完成条数）",
);

// ---- R114 触控右滑完成的判定 ----
check("阈值：刚好 80px 算完成", swipeIntent({ dx: SWIPE_COMPLETE_PX, dy: 0 }), "complete");
check("阈值：79px 还不够", swipeIntent({ dx: SWIPE_COMPLETE_PX - 1, dy: 0 }), "none");
check("左滑不触发（没有反向功能）", swipeIntent({ dx: -200, dy: 0 }), "none");
check("竖向为主算滚动，不算完成", swipeIntent({ dx: 200, dy: 200 }), "none");
check("斜着滑但竖向更大 → 不算", swipeIntent({ dx: 90, dy: 120 }), "none");
check("斜着滑但横向更大 → 算", swipeIntent({ dx: 120, dy: 60 }), "complete");
check("非法数字不触发", [swipeIntent({ dx: Number.NaN, dy: 0 }), swipeIntent({})], ["none", "none"]);
check("跟手位移只跟右滑", [swipeOffset(50), swipeOffset(-50), swipeOffset(Number.NaN)], [50, 0, 0]);
check("跟手位移夹上限（不跑出容器）", swipeOffset(500), 120);
check("自定义上限", swipeOffset(90, 60), 60);

// ---- R108 预览路径拼接（后端只放行 vault 目录下的证据文件） ----
check("目录 + 文件名", vaultFilePath("D:\\vault", "inbox.md.corrupt-1"), "D:\\vault\\inbox.md.corrupt-1");
check("目录带尾反斜杠不会拼出双分隔符", vaultFilePath("D:\\vault\\", "inbox.md.corrupt-1"), "D:\\vault\\inbox.md.corrupt-1");
check("目录带尾斜杠同理", vaultFilePath("D:/vault/", "inbox.md.corrupt-2"), "D:/vault\\inbox.md.corrupt-2");
check("目录为空时只剩文件名（后端会拒绝，先让它自己报）", vaultFilePath("", "inbox.md.corrupt-1"), "inbox.md.corrupt-1");
check("脏入参不炸", [vaultFilePath(null, "a"), vaultFilePath("D:\\v", null)], ["a", "D:\\v\\"]);
// Linux/macOS 绝对路径必须用正斜杠拼接：反斜杠在那里是文件名字符的一部分，
// 拼出来的路径后端会因为"不在 vault 目录下"而拒绝，预览永远打不开。
check(
  "Linux 绝对路径用正斜杠拼接",
  vaultFilePath("/home/u/Documents/Slip", "inbox.md.corrupt-1"),
  "/home/u/Documents/Slip/inbox.md.corrupt-1",
);
check(
  "Linux 目录带尾斜杠不会拼出双分隔符",
  vaultFilePath("/home/u/Documents/Slip/", "inbox.md.corrupt-2"),
  "/home/u/Documents/Slip/inbox.md.corrupt-2",
);
check(
  "相对路径（不含盘符/反斜杠）也按正斜杠处理",
  vaultFilePath("Slip", "inbox.md.corrupt-3"),
  "Slip/inbox.md.corrupt-3",
);
check(
  "Linux 文件名里含反斜杠时也不误判成 Windows",
  vaultFilePath("/home/u/My\\Vault", "inbox.md.corrupt-4"),
  "/home/u/My\\Vault/inbox.md.corrupt-4",
);

// ---- 时间解析：任意位数的小数秒都必须能解析（部分 WebKit 只吃 3 位） ----
const ISO_3 = "2026-11-12T07:05:09.123Z";
const ISO_6 = "2026-11-12T07:05:09.123456Z";
const ISO_9 = "2026-11-12T07:05:09.123456789Z";
check("3 位小数秒解析", Number.isNaN(Date.parse(ISO_3)), false);
check("6 位小数秒也能给出相对时间", relativeTime(ISO_6, Date.parse(ISO_3) + 1000) !== "", true);
check("9 位小数秒也能给出相对时间", relativeTime(ISO_9, Date.parse(ISO_3) + 1000) !== "", true);
check(
  "9 位小数秒截断到毫秒（与 3 位结果一致）",
  relativeTime(ISO_9, Date.parse(ISO_3) + 1000),
  relativeTime(ISO_3, Date.parse(ISO_3) + 1000),
);
check("9 位小数秒的精确时间不为空", exactLocal(ISO_9) !== "", true);

// Node / Bun 的 Date.parse 本身就接受 9 位小数秒，所以上面的断言在本地**测不出**
// WebKit 的差异。这里把 Date.parse 临时替换成"只吃 ≤3 位小数秒"的模拟实现，
// 才能证明截断确实发生在解析之前——去掉 parseIso 里的 replace，这三条会立刻变红。
const nativeParse = Date.parse;
Date.parse = (value) =>
  typeof value === "string" && /\.\d{4,}/.test(value) ? Number.NaN : nativeParse(value);
try {
  check("模拟 WebKit：9 位小数秒仍能算出相对时间", relativeTime(ISO_9, nativeParse(ISO_3) + 1000) !== "", true);
  check("模拟 WebKit：9 位小数秒仍能给出精确时间", exactLocal(ISO_9) !== "", true);
  check("模拟 WebKit：今日统计用的 parseIso 不会返回 NaN", Number.isNaN(parseIso(ISO_9)), false);
} finally {
  Date.parse = nativeParse;
}
check("无小数秒仍照常解析", relativeTime("2026-11-12T07:05:09Z", Date.parse(ISO_3) + 1000), "刚刚");
check("无法解析的字符串仍是空串", relativeTime("not-a-time", Date.parse(ISO_3)), "");

// ---- R119 收起淡出的延迟 ----
check("正常情况等一次 0.12s 淡出", closeFadeDelay(false), CLOSE_FADE_MS);
check("未声明 reduce 时按正常处理", closeFadeDelay(undefined), CLOSE_FADE_MS);
check("要求减少动效时立即消失（延迟为 0）", closeFadeDelay(true), 0);

// ---- R104 归档抽屉：容量摘要、彻底删除的二次确认与"不给撤销"语义 ----
// 摘要：拿不到容量就**不显示**（空串），绝不用 0 冒充——抽屉里明明有记录时，
// "归档 0 条"是错误信息。
check("容量摘要：条数与取整后的 KB", capacitySummary({ archived: 3, archivedBytes: 2048 }), "归档 3 条，约占 2 KB");
check("容量摘要：四舍五入到整数 KB", capacitySummary({ archived: 3, archivedBytes: 1500 }), "归档 3 条，约占 1 KB");
check("容量摘要：1536 B 进位到 2 KB", capacitySummary({ archived: 1, archivedBytes: 1536 }), "归档 1 条，约占 2 KB");
check("容量摘要：几十字节也不说成 0 KB", capacitySummary({ archived: 1, archivedBytes: 40 }), "归档 1 条，约占 1 KB");
check("容量摘要：没有字节数时只报条数", capacitySummary({ archived: 2 }), "归档 2 条");
check(
  "容量摘要：拿不到容量就不显示",
  [capacitySummary(null), capacitySummary(undefined), capacitySummary({}), capacitySummary({ archived: Number.NaN })],
  ["", "", "", ""],
);
check("容量摘要：零归档不显示", capacitySummary({ archived: 0, archivedBytes: 0 }), "");
check("容量摘要：脏字节数只报条数", capacitySummary({ archived: 2, archivedBytes: -5 }), "归档 2 条");

// 彻底删除后的提示：**不能**挂撤销（后端成功时已清空撤销栈），并说明不可逆。
const purged = purgeNotice();
check("彻底删除的提示明确不可逆", purged.message.includes("无法恢复"), true);
check("彻底删除的提示说明已留备份", purged.message.includes("备份"), true);
check("彻底删除不挂撤销按钮（否则是死按钮）", purged.undo, false);
check("彻底删除的提示可以被 Alt+Z 收起（纯信息）", noticeAction(purged), "dismiss");

// 二次确认的状态迁移：武装 → 窗口内执行 → 超时自动还原（R104 复用的闸门）
check("第一下武装并记下时间", confirmNext(null, 1000), { armedAt: 1000, execute: false });
check("窗口内第二下执行并解除武装", confirmNext(1000, 1000 + CONFIRM_WINDOW_MS), { armedAt: null, execute: true });
check(
  "窗口内普通第二下执行",
  confirmNext(1000, 1500),
  { armedAt: null, execute: true },
);
check(
  "超时后不是执行而是重新武装（自动还原）",
  confirmNext(1000, 1000 + CONFIRM_WINDOW_MS + 1),
  { armedAt: 1000 + CONFIRM_WINDOW_MS + 1, execute: false },
);
check("时钟回拨同样退回武装", confirmNext(9000, 1000), { armedAt: 1000, execute: false });
check("自定义窗口：500ms 内仍执行", confirmNext(0, 400, 500).execute, true);
check("自定义窗口：超出就重新武装", confirmNext(0, 501, 500), { armedAt: 501, execute: false });
// 一条完整的点两下叙事：第一下武装、第二下执行
const gate1 = confirmNext(null, 10_000);
const gate2 = confirmNext(gate1.armedAt, 12_000);
check("点两下才会真的执行", [gate1.execute, gate2.execute], [false, true]);

// ---------- R118：列表边界不蜂鸣 ----------
//
// 边界处的正确行为是"什么也不做"，而且调用方必须已经 preventDefault——
// 否则浏览器会滚动容器（Windows 上很多场景伴随一声系统提示音）。
// 这里钉住"返回 null"，也就是"别移动、别滚动"。
check("首行按 ↑ 不移动（边界返回 null）", listNavTarget(0, 5, "ArrowUp"), null);
check("末行按 ↓ 不移动（边界返回 null）", listNavTarget(4, 5, "ArrowDown"), null);
check("单行列表两个方向都到边界", [listNavTarget(0, 1, "ArrowUp"), listNavTarget(0, 1, "ArrowDown")], [null, null]);
check("空列表任何方向都不动", listNavTarget(0, 0, "ArrowDown"), null);

// ---------- R117：浅色模式下边框的非文字对比度 ----------
//
// 发丝边框属于 WCAG 1.4.11 的"非文字 UI 组件"，下限是 3:1。
// 这条断言守的是"浅色模式放在深色壁纸上时，纸条边界仍然看得见"。
const lightPaper = "#fbf7ee";
const lightEdge = "#e2d9c6";
const lightFocusRing = "#a8552c";
const lightInkSoft = "#787061";
const darkPaper = "#201e1b";
const darkEdge = "#3c3831";
const darkFocusRing = "#f0a173";
const darkInkSoft = "#a89f8d";

// 先说结论：**发丝边框达不到 WCAG 3:1，这是有意的**。
//
// 实测 1.31（浅色）/ 1.43（深色）。它属于"纯装饰"——纸条是靠正文、复选框、
// hover 与焦点环被识别出来的，边框只负责"纸感"。WCAG 1.4.11 的 3:1 针对
// **识别控件所必需的**视觉信息，装饰性分隔线不在其列。
//
// 但"有意为之"不等于"随便它漂"：这里把实测值当**下界**钉住，
// 防止后续调样式时不知不觉把边框调到跟背景一样（那才是真的看不见）。
check("浅色边框对比度不低于 1.25（装饰性，低于 3:1 是有意取舍）", contrastRatio(lightEdge, lightPaper) >= 1.25, true);
check("深色边框对比度不低于 1.35（同上）", contrastRatio(darkEdge, darkPaper) >= 1.35, true);

// 真正必须合规的是**交互线索**：焦点环 ≥ 3:1（1.4.11），次要文字 ≥ 4.5:1（1.4.3）。
check("浅色焦点环对比度 ≥ 3（1.4.11，这才是必须合规的那条）", contrastRatio(lightFocusRing, lightPaper) >= 3, true);
check("深色焦点环对比度 ≥ 3", contrastRatio(darkFocusRing, darkPaper) >= 3, true);
check("浅色次要文字对比度 ≥ 4.5（1.4.3）", contrastRatio(lightInkSoft, lightPaper) >= 4.5, true);
check("深色次要文字对比度 ≥ 4.5", contrastRatio(darkInkSoft, darkPaper) >= 4.5, true);

// 计算器自身的正确性：黑白是 21:1，同色是 1:1。
check("黑白对比度为 21", Math.round(contrastRatio("#000000", "#ffffff")), 21);
check("同色对比度为 1", contrastRatio("#123456", "#123456"), 1);
check("非法颜色返回 null", [parseHex("red"), contrastRatio("zzz", "#fff")], [null, null]);
check("相对亮度：白 1 / 黑 0", [Math.round(relativeLuminance("#ffffff")), relativeLuminance("#000000")], [1, 0]);

// ---------- 退出判定必须真的在最后 ----------
//
// 这条守卫守的是**脚本自身**：退出判定曾经写在文件中间，其后还有 14 条 check(
// （列表边界 + 对比度），于是那 14 条即使全红进程也照样 exit 0——
// 实测过：往末尾追加一条必失败断言，退出码仍是 0。
//
// 所以这里读自己的源码，确认「最后一次 check( 调用」出现在退出判定之前。
// 将来任何人往末尾加断言忘了挪判定，这条会立刻变红。
{
  const source = await readFile(new URL(import.meta.url), "utf8");
  const lines = source.split("\n");
  const lastCheck = lines.reduce(
    (last, line, index) => (/(^|[^.\w])check\s*\(/.test(line) ? index : last),
    -1,
  );
  const gate = lines.findIndex((line) => line.includes("process.exit(1)") && line.includes("if"));
  const gateLine = lines.findIndex((line) => line.trim() === "if (failed > 0) {");
  const effectiveGate = gateLine >= 0 ? gateLine : gate;
  check("退出判定存在", effectiveGate >= 0, true);
  check(
    "最后一次 check( 在退出判定之前（否则末尾断言永远不参与判定）",
    lastCheck >= 0 && effectiveGate > lastCheck,
    true,
  );
  if (lastCheck >= 0 && effectiveGate <= lastCheck) {
    console.log(
      `  最后一次 check( 在第 ${lastCheck + 1} 行，退出判定在第 ${effectiveGate + 1} 行`,
    );
  }
}

if (failed > 0) {
  console.log(`\n失败 ${failed} 项。`);
  process.exit(1);
}
console.log("\n全部通过。");
