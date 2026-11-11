#!/usr/bin/env bun
// 前端纯逻辑的检查（不需要启动应用、不需要浏览器）。
//
// 为什么单独有它：像 `bannerLevel` 这样的判定是**纯函数**，但要验它就得把 GUI 跑起来
// 做 CDP 取证——而本机 Smart App Control 时不时拒绝执行新编出的产物。把这些判定抽出来
// 之后，一条 `bun` 命令就能验，不受环境影响。
//
// 用法：& "C:\Users\spand\.bun\bin\bun.exe" run tools/check-ui-logic.mjs
import { bannerLevel } from "../ui/src/lib/status.js";
import { createSerialQueue, nextToggleTarget } from "../ui/src/lib/queue.js";
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
} from "../ui/src/lib/keys.js";

let failed = 0;
const check = (label, actual, expected) => {
  const ok = JSON.stringify(actual) === JSON.stringify(expected);
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

if (failed > 0) {
  console.log(`\n失败 ${failed} 项。`);
  process.exit(1);
}
console.log("\n全部通过。");
