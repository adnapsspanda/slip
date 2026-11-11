#!/usr/bin/env bun
// 长列表渲染的 DOM 与内存检查（R73）。
//
// 用法：先带 CDP 端口启动 slip-shell，再跑
//   & "C:\Users\spand\.bun\bin\bun.exe" run tools/cdp-memory.mjs [端口] [条数]
//
// 检查什么：
//   1. 500 条时 DOM 节点数是否随条数线性增长（不是平方，也没有一堆包装节点）；
//   2. 反复切换视图（归档抽屉 / 过滤）20 次之后，JS 堆是否明显增长——增长说明有闭包或
//      监听没被释放。
//
// 为什么单独一个脚本而不是塞进 cdp-probe：这两项要读堆、要强制 GC，属于"体检"而不是
// "功能断言"，混进去会让功能探针变慢且更容易受环境影响。

const port = process.argv[2] ?? "9222";
const expected = Number(process.argv[3] ?? "500");

const targets = await (await fetch(`http://127.0.0.1:${port}/json`)).json();
const page = targets.find((t) => t.type === "page");
if (!page) {
  console.error("没有找到页面目标——应用起来了吗？端口对吗？");
  process.exit(2);
}

const ws = new WebSocket(page.webSocketDebuggerUrl);
await new Promise((resolve) => (ws.onopen = resolve));
let seq = 0;
const pending = new Map();
ws.onmessage = (event) => {
  const data = JSON.parse(event.data);
  if (pending.has(data.id)) {
    pending.get(data.id)(data.result);
    pending.delete(data.id);
  }
};
const send = (method, params) =>
  new Promise((resolve) => {
    const id = ++seq;
    pending.set(id, resolve);
    ws.send(JSON.stringify({ id, method, params }));
  });

const evaluate = async (expression, awaitPromise = false) => {
  const result = await send("Runtime.evaluate", {
    expression,
    returnByValue: true,
    awaitPromise,
  });
  return result.result?.value;
};

let failed = 0;
const check = (label, ok, detail = "") => {
  if (!ok) failed += 1;
  console.log(`${ok ? "[PASS]" : "[FAIL]"} ${label}${detail ? ` - ${detail}` : ""}`);
};

await send("Runtime.enable", {});
await send("HeapProfiler.enable", {});

const settle = () => new Promise((r) => setTimeout(r, 400));
const collectGarbage = async () => {
  await send("HeapProfiler.collectGarbage", {});
  await new Promise((r) => setTimeout(r, 250));
};
const heapMB = async () => {
  const usage = await send("Runtime.getHeapUsage", {});
  return Math.round((usage.usedSize / (1024 * 1024)) * 100) / 100;
};
const nodeCount = () => evaluate(`document.querySelectorAll('*').length`);
const rowCount = () => evaluate(`document.querySelectorAll('.slips .slip').length`);

// ---- 1. 条数与 DOM 节点 ----
const rows = await rowCount();
console.log(`\n=== 长列表渲染检查（期望 ${expected} 条）===`);
check(`列表渲染出 ${expected} 条`, rows === expected, `实际 ${rows}`);

const nodes = await nodeCount();
// 每条记录当前约 4–6 个节点（li / 复选框 / 正文 span / 操作按钮）。给到 15 是留足余量，
// 但仍能抓住"每条渲染出几十个节点"这类失控。
const perRow = rows > 0 ? nodes / rows : 0;
check(
  "DOM 节点数随条数线性（每条 < 15 个）",
  perRow < 15,
  `节点 ${nodes}，每条 ${perRow.toFixed(1)}`,
);

// ---- 2. 反复重建视图，看堆是否增长 ----
await collectGarbage();
const before = await heapMB();
console.log(`\n=== 视图反复重建 20 次 ===\n初始堆 ${before} MB`);

for (let i = 0; i < 20; i += 1) {
  // 过滤 → 清空：每次都会让 Svelte 重建整列
  await evaluate(`(() => {
    const input = document.querySelector('.filter__input');
    if (input) {
      const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value').set;
      setter.call(input, '不存在的过滤词');
      input.dispatchEvent(new Event('input', { bubbles: true }));
    }
    return true;
  })()`);
  await settle();
  await evaluate(`(() => {
    const input = document.querySelector('.filter__input');
    if (input) {
      const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value').set;
      setter.call(input, '');
      input.dispatchEvent(new Event('input', { bubbles: true }));
    }
    return true;
  })()`);
  await settle();
}

await collectGarbage();
const after = await heapMB();
const growth = Math.round((after - before) * 100) / 100;
console.log(`重建后堆 ${after} MB（增长 ${growth} MB）`);

// 20 次重建若每次泄漏一点点闭包，增长会很可观；这里给 8 MB 的宽容度，
// 主要抓"每次重建都留下完整一份列表"这种量级的泄漏。
check("反复重建后堆没有明显增长（< 8 MB）", growth < 8, `增长 ${growth.toFixed(2)} MB`);

const rowsAfter = await rowCount();
check("重建后条数不变", rowsAfter === expected, `实际 ${rowsAfter}`);

ws.close();
console.log(failed === 0 ? "\n全部通过。" : `\n失败 ${failed} 项。`);
process.exit(failed === 0 ? 0 : 1);
