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

// 4. 超过单条上限时回车不提交，且提示要指出是**哪一行**（回归红线）。
//    后端只说"太长"，说不清第几行、超多少，所以前端这层提示是有意义的。
const tooLong = JSON.parse(
  await evaluate(
    `(async () => {
       const ta = document.querySelector('.capture__input');
       const before = document.querySelectorAll('.slip').length;
       ta.focus();
       ta.value = 'ok\\n' + 'a'.repeat(2001);
       ta.dispatchEvent(new Event('input', { bubbles: true }));
       await new Promise(r => setTimeout(r, 150));
       const hintBefore = document.querySelector('.capture__hint')?.textContent ?? '';
       ta.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
       await new Promise(r => setTimeout(r, 600));
       const keptLen = ta.value.length;
       const count = document.querySelectorAll('.slip').length;
       ta.value = '';
       ta.dispatchEvent(new Event('input', { bubbles: true }));
       return JSON.stringify({ hintBefore, keptLen, count, before });
     })()`,
    true,
  ),
);
check(
  "超长时回车不提交且草稿保留",
  tooLong.count === tooLong.before && tooLong.keptLen > 0,
  `before=${tooLong.before} after=${tooLong.count} keptLen=${tooLong.keptLen}`,
);
check(
  "超长提示指出是哪一行",
  (tooLong.hintBefore ?? "").includes("第 2 行"),
  `hint=${tooLong.hintBefore}`,
);

// 5. 双击正文 → 行内编辑框 IME 组合态按 Enter / Esc 不得误提交或丢弃 → 回车提交。
const editIme = JSON.parse(
  await evaluate(
    `(async () => {
       const text = document.querySelector('.slip__text');
       if (!text) return JSON.stringify({ error: '没有纸条' });
       text.dispatchEvent(new MouseEvent('dblclick', { bubbles: true }));
       await new Promise(r => setTimeout(r, 250));
       const input = document.querySelector('.slip__edit');
       if (!input) return JSON.stringify({ error: '编辑框没有出现' });
       input.value = '拼音选词中';
       input.dispatchEvent(new Event('input', { bubbles: true }));
       input.dispatchEvent(new CompositionEvent('compositionstart'));
       input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true, isComposing: true }));
       input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true, isComposing: true }));
       await new Promise(r => setTimeout(r, 300));
       const stillEditing = document.querySelector('.slip__edit') !== null;
       const draftVal = input.value;
       input.dispatchEvent(new CompositionEvent('compositionend'));
       return JSON.stringify({ stillEditing, draftVal });
     })()`,
    true,
  ),
);
check(
  "行内编辑框 IME 组合态按 Enter/Esc 不提交不退出",
  editIme.stillEditing === true && editIme.draftVal === "拼音选词中",
  `stillEditing=${editIme.stillEditing} draftVal=${editIme.draftVal}`,
);

const edited = JSON.parse(
  await evaluate(
    `(async () => {
       const input = document.querySelector('.slip__edit');
       if (!input) return JSON.stringify({ error: '编辑框没有出现' });
       input.value = '编辑后的内容';
       input.dispatchEvent(new Event('input', { bubbles: true }));
       input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
       await new Promise(r => setTimeout(r, 1500));
       return JSON.stringify({
         texts: [...document.querySelectorAll('.slip__text')].map(e => e.textContent),
         notice: document.querySelector('.notice')?.textContent ?? '',
         cleared: document.querySelector('.slip__edit') === null,
       });
     })()`,
    true,
  ),
);
check(
  "双击进入编辑并回车提交",
  (edited.texts ?? []).includes("编辑后的内容"),
  `texts=${JSON.stringify(edited.texts)}`,
);
check("提交后编辑框退出", edited.cleared === true, `cleared=${edited.cleared}`);
check("提示区出现已修改", (edited.notice ?? "").includes("已修改"), `notice=${edited.notice}`);

// 6. Ctrl+Z 走窗口级快捷键，回滚到上一次正文。
const undone = JSON.parse(
  await evaluate(
    `(async () => {
       window.dispatchEvent(new KeyboardEvent('keydown', { key: 'z', ctrlKey: true, bubbles: true }));
       await new Promise(r => setTimeout(r, 1500));
       return JSON.stringify({
         texts: [...document.querySelectorAll('.slip__text')].map(e => e.textContent),
         notice: document.querySelector('.notice')?.textContent ?? '',
       });
     })()`,
    true,
  ),
);
check(
  "Ctrl+Z 回滚上一次编辑",
  (undone.texts ?? []).includes("热键唤出后写入的一条") &&
    !(undone.texts ?? []).includes("编辑后的内容"),
  `texts=${JSON.stringify(undone.texts)}`,
);
check("提示区出现已撤销", (undone.notice ?? "").includes("已撤销"), `notice=${undone.notice}`);

// 7. 点锁定按钮：标记落到 marker 上，正文转次级色。
const lockedState = JSON.parse(
  await evaluate(
    `(async () => {
       const btn = document.querySelector('.slip__lock');
       if (!btn) return JSON.stringify({ error: '没有锁定按钮' });
       btn.click();
       await new Promise(r => setTimeout(r, 1500));
       const text = document.querySelector('.slip__text');
       return JSON.stringify({
         pressed: document.querySelector('.slip__lock')?.getAttribute('aria-pressed'),
         lockedClass: text?.className ?? '',
       });
     })()`,
    true,
  ),
);
check("点锁定后按钮转为按下态", lockedState.pressed === "true", `pressed=${lockedState.pressed}`);
check(
  "锁定后正文使用次级样式",
  (lockedState.lockedClass ?? "").includes("slip__text--locked"),
  `class=${lockedState.lockedClass}`,
);

// 8. 已锁定的纸条双击不进入编辑，并说明原因。
const lockedEdit = JSON.parse(
  await evaluate(
    `(async () => {
       const text = document.querySelector('.slip__text');
       text.dispatchEvent(new MouseEvent('dblclick', { bubbles: true }));
       await new Promise(r => setTimeout(r, 400));
       return JSON.stringify({
         hasEdit: document.querySelector('.slip__edit') !== null,
         hint: document.querySelector('.capture__hint')?.textContent ?? '',
       });
     })()`,
    true,
  ),
);
check("锁定后双击不进入编辑", lockedEdit.hasEdit === false, `hasEdit=${lockedEdit.hasEdit}`);
check(
  "锁定后双击给出原因",
  (lockedEdit.hint ?? "").includes("已锁定"),
  `hint=${lockedEdit.hint}`,
);

// 9. 锁只锁正文：勾选必须照常可用，否则锁会阻止任务完成。
const doneWhileLocked = JSON.parse(
  await evaluate(
    `(async () => {
       const box = document.querySelector('.slip__box');
       box.click();
       await new Promise(r => setTimeout(r, 1500));
       return JSON.stringify({
         checked: document.querySelector('.slip__box')?.getAttribute('aria-checked'),
         notice: document.querySelector('.notice')?.textContent ?? '',
       });
     })()`,
    true,
  ),
);
check(
  "锁定的纸条仍然可以勾选完成",
  doneWhileLocked.checked === "true",
  `aria-checked=${doneWhileLocked.checked}`,
);
check(
  "勾选完成后提示区出现已完成与撤销入口",
  (doneWhileLocked.notice ?? "").includes("已完成") &&
    (doneWhileLocked.notice ?? "").includes("撤销"),
  `notice=${doneWhileLocked.notice}`,
);

// 10. 归档已完成纸条 → 从可见列表收起 → Ctrl+Z 撤销归档恢复到列表。
const archivedAndUndone = JSON.parse(
  await evaluate(
    `(async () => {
       const btn = document.querySelector('.slip__archive');
       if (!btn) return JSON.stringify({ error: '没有归档按钮' });
       btn.click();
       await new Promise(r => setTimeout(r, 1500));
       const countAfterArchive = document.querySelectorAll('.slip').length;
       const noticeAfterArchive = document.querySelector('.notice')?.textContent ?? '';
       window.dispatchEvent(new KeyboardEvent('keydown', { key: 'z', ctrlKey: true, bubbles: true }));
       await new Promise(r => setTimeout(r, 1500));
       const countAfterUndo = document.querySelectorAll('.slip').length;
       const checkedAfterUndo = document.querySelector('.slip__box')?.getAttribute('aria-checked');
       return JSON.stringify({
         countAfterArchive,
         noticeAfterArchive,
         countAfterUndo,
         checkedAfterUndo,
       });
     })()`,
    true,
  ),
);
check(
  "归档后纸条从默认列表收起",
  archivedAndUndone.countAfterArchive === 0 &&
    (archivedAndUndone.noticeAfterArchive ?? "").includes("已归档"),
  `count=${archivedAndUndone.countAfterArchive} notice=${archivedAndUndone.noticeAfterArchive}`,
);
check(
  "Ctrl+Z 可撤销归档并保留已完成状态",
  archivedAndUndone.countAfterUndo === 1 &&
    archivedAndUndone.checkedAfterUndo === "true",
  `count=${archivedAndUndone.countAfterUndo} checked=${archivedAndUndone.checkedAfterUndo}`,
);

cdp.close();
process.exit(failed ? 1 : 0);
