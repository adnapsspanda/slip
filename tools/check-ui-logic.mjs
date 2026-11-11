#!/usr/bin/env bun
// 前端纯逻辑的检查（不需要启动应用、不需要浏览器）。
//
// 为什么单独有它：像 `bannerLevel` 这样的判定是**纯函数**，但要验它就得把 GUI 跑起来
// 做 CDP 取证——而本机 Smart App Control 时不时拒绝执行新编出的产物。把这些判定抽出来
// 之后，一条 `bun` 命令就能验，不受环境影响。
//
// 用法：& "C:\Users\spand\.bun\bin\bun.exe" run tools/check-ui-logic.mjs
import { bannerLevel } from "../ui/src/lib/status.js";
import { submitIntent, listNavTarget } from "../ui/src/lib/keys.js";

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

if (failed > 0) {
  console.log(`\n失败 ${failed} 项。`);
  process.exit(1);
}
console.log("\n全部通过。");
