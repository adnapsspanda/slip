#!/usr/bin/env bun
// 前端纯逻辑的检查（不需要启动应用、不需要浏览器）。
//
// 为什么单独有它：像 `bannerLevel` 这样的判定是**纯函数**，但要验它就得把 GUI 跑起来
// 做 CDP 取证——而本机 Smart App Control 时不时拒绝执行新编出的产物。把这些判定抽出来
// 之后，一条 `bun` 命令就能验，不受环境影响。
//
// 用法：& "C:\Users\spand\.bun\bin\bun.exe" run tools/check-ui-logic.mjs
import { bannerLevel } from "../ui/src/lib/status.js";

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

if (failed > 0) {
  console.log(`\n失败 ${failed} 项。`);
  process.exit(1);
}
console.log("\n全部通过。");
