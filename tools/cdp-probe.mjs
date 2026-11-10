// 通过 WebView2 的 CDP 端点驱动真实界面。
//
// 配合 tools/verify-hotkey.ps1 使用：它负责发真实热键，这里负责回答
// "界面到底变成了什么样"——因为截屏和"看起来对"都不算证据。
//
// 用法：bun run tools/cdp-probe.mjs [port]

const port = process.argv[2] ?? "9222";
// --dismiss：只把窗口收走就退出，用于验收脚本准备状态。
const dismissOnly = process.argv.includes("--dismiss");
const ENDPOINT = `http://127.0.0.1:${port}`;

function connect(url) {
  return new Promise((resolve, reject) => {
    const socket = new WebSocket(url);
    let counter = 0;
    const pending = new Map();

    socket.onopen = () =>
      resolve({
        send(method, params) {
          const id = ++counter;
          return new Promise((res, rej) => {
            pending.set(id, { res, rej });
            socket.send(JSON.stringify({ id, method, params }));
          });
        },
        close: () => socket.close(),
      });

    socket.onerror = (e) => reject(new Error(`ws: ${e.message}`));

    socket.onmessage = (e) => {
      const data = JSON.parse(e.data);
      if (data.id && pending.has(data.id)) {
        const { res, rej } = pending.get(data.id);
        pending.delete(data.id);
        data.error ? rej(new Error(JSON.stringify(data.error))) : res(data.result);
      }
    };
  });
}

const targets = await (await fetch(`${ENDPOINT}/json`)).json();
const page = targets.find((t) => t.type === "page");
if (!page) {
  console.log("找不到 page target，CDP 不可用");
  process.exit(1);
}

const cdp = await connect(page.webSocketDebuggerUrl);
await cdp.send("Runtime.enable", {});

async function evaluate(expression, awaitPromise = false) {
  const result = await cdp.send("Runtime.evaluate", {
    expression,
    returnByValue: true,
    awaitPromise,
  });
  if (result.exceptionDetails) {
    return JSON.stringify({ __exception: result.exceptionDetails.text });
  }
  return result.result.value;
}

if (dismissOnly) {
  await evaluate(`window.__TAURI__.core.invoke('dismiss_window')`, true);
  cdp.close();
  process.exit(0);
}

let failed = false;
const check = (label, ok, detail) => {
  if (!ok) failed = true;
  console.log(`${ok ? "[PASS]" : "[FAIL]"} ${label}${detail ? ` — ${detail}` : ""}`);
};

// 1. 热键唤出后，捕获框必须真的拿到焦点。
const awake = JSON.parse(
  await evaluate(`JSON.stringify({
    visible: document.visibilityState,
    tag: document.activeElement?.tagName ?? null,
    cls: document.activeElement?.className ?? null,
    tauri: Boolean(window.__TAURI__),
  })`),
);
check("唤出后捕获框获得焦点", awake.cls === "capture__input", `activeElement=${awake.tag}.${awake.cls}`);
check("Tauri 桥可用", awake.tauri === true);

// 2. 真打一条并按回车，走完整 IPC -> 领域 -> 磁盘 -> 快照。
const typed = JSON.parse(
  await evaluate(
    `(async () => {
       const ta = document.querySelector('.capture__input');
       if (!ta) return JSON.stringify({ error: 'no input' });
       ta.focus();
       ta.value = '热键唤出后写入的一条';
       ta.dispatchEvent(new Event('input', { bubbles: true }));
       ta.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
       await new Promise(r => setTimeout(r, 1500));
       return JSON.stringify({
         hint: document.querySelector('.capture__hint')?.textContent ?? null,
         slips: [...document.querySelectorAll('.slip__text')].map(e => e.textContent),
         inputLeft: ta.value,
       });
     })()`,
    true,
  ),
);
check("回车后提示为已收录", typed.hint === "已收录", `hint=${typed.hint}`);
check("列表出现该条", (typed.slips ?? []).includes("热键唤出后写入的一条"), `slips=${JSON.stringify(typed.slips)}`);
check("输入框已清空", typed.inputLeft === "", `left=${JSON.stringify(typed.inputLeft)}`);

// 3. 输入法组合态按回车必须不提交（回归红线）。
const ime = JSON.parse(
  await evaluate(
    `(async () => {
       const ta = document.querySelector('.capture__input');
       const before = document.querySelectorAll('.slip').length;
       ta.focus();
       ta.value = '半截拼音';
       ta.dispatchEvent(new Event('input', { bubbles: true }));
       ta.dispatchEvent(new CompositionEvent('compositionstart'));
       ta.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true, isComposing: true }));
       await new Promise(r => setTimeout(r, 600));
       const kept = ta.value;
       const count = document.querySelectorAll('.slip').length;
       ta.dispatchEvent(new CompositionEvent('compositionend'));
       ta.value = '';
       ta.dispatchEvent(new Event('input', { bubbles: true }));
       return JSON.stringify({ kept, count, before });
     })()`,
    true,
  ),
);
check(
  "IME 组合态回车不提交",
  ime.kept === "半截拼音" && ime.count === ime.before,
  `inputKept=${ime.kept} before=${ime.before} after=${ime.count}`,
);

cdp.close();
process.exit(failed ? 1 : 0);
