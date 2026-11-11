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

// 1.5 空 vault 的首屏必须有指引。第一次启动那张空纸最容易被误解成"什么也没发生"，
//     所以这一步在录入之前断言，且要求指引里含可识别的按键说明而不只是一句空话。
const emptyState = JSON.parse(
  await evaluate(`JSON.stringify({
    present: document.querySelector('.slips__empty') !== null,
    title: document.querySelector('.slips__empty-title')?.textContent?.trim() ?? '',
    steps: document.querySelectorAll('.slips__empty-steps li').length,
    keys: [...document.querySelectorAll('.slips__empty kbd')].map(e => e.textContent),
  })`),
);
check("空 vault 时展示空态指引", emptyState.present === true && emptyState.title.length > 0,
  `title=${emptyState.title}`);
check("空态指引给出了两步操作与按键", emptyState.steps === 2 && emptyState.keys.length >= 3,
  `steps=${emptyState.steps} keys=${JSON.stringify(emptyState.keys)}`);

// 1.6 干净 vault 不该出现损坏文件角标。这条同时验证 `corrupt_files` 的契约：
//     返回数组（不是 null/对象），否则 `corrupt.length` 会在界面上炸成 TypeError。
const corruptContract = JSON.parse(
  await evaluate(`(async () => {
    const list = await window.__TAURI__.core.invoke('corrupt_files');
    return JSON.stringify({
      isArray: Array.isArray(list),
      length: Array.isArray(list) ? list.length : -1,
      badgeShown: document.querySelector('.paper__link') !== null,
    });
  })()`, true),
);
check("corrupt_files 返回数组", corruptContract.isArray === true, `isArray=${corruptContract.isArray}`);
check("干净 vault 不显示损坏角标",
  corruptContract.length === 0 && corruptContract.badgeShown === false,
  `length=${corruptContract.length} badge=${corruptContract.badgeShown}`);

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
         emptyGone: document.querySelector('.slips__empty') === null,
       });
     })()`,
    true,
  ),
);
check("回车后提示为已收录", typed.hint === "已收录", `hint=${typed.hint}`);
check("列表出现该条", (typed.slips ?? []).includes("热键唤出后写入的一条"), `slips=${JSON.stringify(typed.slips)}`);
check("输入框已清空", typed.inputLeft === "", `left=${JSON.stringify(typed.inputLeft)}`);
check("录入后空态引导无缝让位给列表", typed.emptyGone === true, `emptyGone=${typed.emptyGone}`);

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

// 11. 全键盘流：录入 → ↓ 进列表 → Space 勾选 → Enter 编辑 → Esc 退出 → ↑ 回捕获框。
//     放在最后，因为前面的用例已经把第一条锁定并把状态改了很多轮。
const keyboardFlow = JSON.parse(
  await evaluate(
    `(async () => {
       const ta = document.querySelector('.capture__input');
       // 先录一条新的：它未锁定，才能测 Enter 编辑。
       ta.focus();
       ta.value = '键盘流测试的一条';
       ta.dispatchEvent(new Event('input', { bubbles: true }));
       ta.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
       await new Promise(r => setTimeout(r, 1500));

       // ↓：输入框清空后按下，焦点应进入列表第一行
       ta.focus();
       ta.value = '';
       ta.dispatchEvent(new Event('input', { bubbles: true }));
       ta.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowDown', bubbles: true }));
       await new Promise(r => setTimeout(r, 150));
       const spans = () => [...document.querySelectorAll('.slip__text')];
       const downFromCapture = document.activeElement === spans()[0];

       // Space：在第二条（未锁定）上勾选
       const second = spans()[1];
       second.focus();
       second.dispatchEvent(new KeyboardEvent('keydown', { key: ' ', bubbles: true }));
       await new Promise(r => setTimeout(r, 1200));
       const boxes = [...document.querySelectorAll('.slip__box')];
       const spaceChecked = boxes[1]?.getAttribute('aria-checked');

       // Enter：进入编辑
       spans()[1].dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
       await new Promise(r => setTimeout(r, 250));
       const enteredEdit = document.querySelector('.slip__edit') !== null;

       // Esc：退出编辑，焦点应回到那一行而不是丢掉
       document.querySelector('.slip__edit')?.dispatchEvent(
         new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }),
       );
       await new Promise(r => setTimeout(r, 250));
       const leftEdit = document.querySelector('.slip__edit') === null;
       const focusBackOnRow = (document.activeElement?.className ?? '').includes('slip__text');

       // ↑：在第一行按上，焦点应回到捕获框
       spans()[0].focus();
       spans()[0].dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowUp', bubbles: true }));
       await new Promise(r => setTimeout(r, 150));
       const backToCapture = (document.activeElement?.className ?? '').includes('capture__input');

       return JSON.stringify({ downFromCapture, spaceChecked, enteredEdit, leftEdit, focusBackOnRow, backToCapture });
     })()`,
    true,
  ),
);
check("↓ 从空输入框进入列表第一行", keyboardFlow.downFromCapture === true,
  `downFromCapture=${keyboardFlow.downFromCapture}`);
check("Space 勾选纸条", keyboardFlow.spaceChecked === "true",
  `aria-checked=${keyboardFlow.spaceChecked}`);
check("Enter 进入编辑", keyboardFlow.enteredEdit === true,
  `enteredEdit=${keyboardFlow.enteredEdit}`);
check("Esc 退出编辑且焦点回到那一行",
  keyboardFlow.leftEdit === true && keyboardFlow.focusBackOnRow === true,
  `leftEdit=${keyboardFlow.leftEdit} focusBack=${keyboardFlow.focusBackOnRow}`);
check("首行 ↑ 回到捕获框", keyboardFlow.backToCapture === true,
  `backToCapture=${keyboardFlow.backToCapture}`);

// 12. 切换编辑目标不丢改动；Ctrl+Enter 也能提交。
//     这条对应实测过的数据丢失 bug：改着 A 又去双击 B，A 的改动被静默丢弃、
//     B 也没进入编辑。根因是提交逻辑读的是可变的 editingId/draft。
const switchEdit = JSON.parse(
  await evaluate(
    `(async () => {
       const wait = (ms) => new Promise(r => setTimeout(r, ms));
       const ta = document.querySelector('.capture__input');
       for (const value of ['切换测试 A', '切换测试 B']) {
         ta.focus();
         ta.value = value;
         ta.dispatchEvent(new Event('input', { bubbles: true }));
         ta.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
         await wait(1300);
       }

       const rows = [...document.querySelectorAll('.slip')].slice(-2);
       const spanA = rows[0].querySelector('.slip__text');
       const spanB = rows[1].querySelector('.slip__text');

       spanA.dispatchEvent(new MouseEvent('dblclick', { bubbles: true }));
       await wait(250);
       const inputA = document.querySelector('.slip__edit');
       if (!inputA) return JSON.stringify({ error: 'A 没有进入编辑' });
       inputA.value = '切换测试 A 改过';
       inputA.dispatchEvent(new Event('input', { bubbles: true }));

       // 不按 Enter，直接切到 B —— 这正是会丢改动的那一步
       spanB.dispatchEvent(new MouseEvent('dblclick', { bubbles: true }));
       await wait(1600);
       const openValue = document.querySelector('.slip__edit')?.value ?? null;
       const texts = [...document.querySelectorAll('.slip__text')].map((e) => e.textContent);

       // 在 B 的编辑框里用 Ctrl+Enter 提交
       const inputB = document.querySelector('.slip__edit');
       if (inputB) {
         inputB.value = '切换测试 B 改过';
         inputB.dispatchEvent(new Event('input', { bubbles: true }));
         inputB.dispatchEvent(
           new KeyboardEvent('keydown', { key: 'Enter', ctrlKey: true, bubbles: true }),
         );
       }
       await wait(1600);
       return JSON.stringify({
         error: null,
         openValue,
         texts,
         editorGone: document.querySelector('.slip__edit') === null,
         afterCtrlEnter: [...document.querySelectorAll('.slip__text')].map((e) => e.textContent),
       });
     })()`,
    true,
  ),
);
check("切换编辑目标时前一条正常保存",
  (switchEdit.texts ?? []).includes("切换测试 A 改过"),
  `texts=${JSON.stringify(switchEdit.texts)}`);
check("切换后新一条进入编辑且内容正确",
  switchEdit.openValue === "切换测试 B",
  `openValue=${switchEdit.openValue}`);
check("Ctrl+Enter 提交",
  switchEdit.editorGone === true &&
    (switchEdit.afterCtrlEnter ?? []).includes("切换测试 B 改过"),
  `editorGone=${switchEdit.editorGone} texts=${JSON.stringify(switchEdit.afterCtrlEnter)}`);

// 13. 过滤：Ctrl+F 展开、输入筛选、Esc 退出；且"没匹配到"不能显示成"还没有内容"。
const filtered = JSON.parse(
  await evaluate(
    `(async () => {
       const wait = (ms) => new Promise(r => setTimeout(r, ms));
       // Ctrl+F 由窗口级快捷键接管
       window.dispatchEvent(new KeyboardEvent('keydown', { key: 'f', ctrlKey: true, bubbles: true }));
       await wait(200);
       const input = document.querySelector('.filter__input');
       if (!input) return JSON.stringify({ error: '过滤框没有展开' });
       const focused = document.activeElement === input;

       // 筛到只剩下那一条
       input.value = '切换测试 A 改过';
       input.dispatchEvent(new Event('input', { bubbles: true }));
       await wait(150);
       const rows = document.querySelectorAll('.slip').length;
       const count = document.querySelector('.filter__count')?.textContent ?? '';

       // 过滤状态下勾选仍然生效
       const box = document.querySelector('.slip__box');
       box?.click();
       await wait(1200);
       const checked = document.querySelector('.slip__box')?.getAttribute('aria-checked');

       // 换成匹配不到的词：必须说"没有匹配"，不能退回首次使用引导
       input.value = '不存在的关键词zzz';
       input.dispatchEvent(new Event('input', { bubbles: true }));
       await wait(150);
       const emptyTitle = document.querySelector('.slips__empty-title')?.textContent ?? '';

       // Esc 退出过滤
       input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
       await wait(150);
       return JSON.stringify({
         error: null,
         focused,
         rows,
         count,
         checked,
         emptyTitle,
         closed: document.querySelector('.filter__input') === null,
       });
     })()`,
    true,
  ),
);
check("Ctrl+F 展开过滤框并获得焦点",
  !filtered.error && filtered.focused === true, `focused=${filtered.focused}`);
check("输入关键字后只剩匹配项",
  filtered.rows === 1 && filtered.count === "1", `rows=${filtered.rows} count=${filtered.count}`);
check("过滤状态下仍能勾选", filtered.checked === "true", `aria-checked=${filtered.checked}`);
check("无匹配时提示的是『没有匹配』而不是空态引导",
  filtered.emptyTitle === "没有匹配的纸条", `title=${filtered.emptyTitle}`);
check("Esc 退出过滤", filtered.closed === true, `closed=${filtered.closed}`);

// 14. 行内渲染（粗体/代码/链接）与相对时间。
//     正文来自 inbox.md——用户可编辑的文件。渲染必须是结构化标签，不能是 HTML 注入。
const inline = JSON.parse(
  await evaluate(
    `(async () => {
       const wait = (ms) => new Promise(r => setTimeout(r, ms));
       const ta = document.querySelector('.capture__input');
       ta.focus();
       ta.value = '见 **重点** 与 \`npm run build\` 见 https://example.com/a?x=1&y=2。';
       ta.dispatchEvent(new Event('input', { bubbles: true }));
       ta.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
       await wait(1600);

       const rows = [...document.querySelectorAll('.slip')];
       const last = rows[rows.length - 1];
       const body = last?.querySelector('.slip__text');
       const time = last?.querySelector('.slip__time');

       // 校验层：open_url 必须拒绝非 http/https
       let rejected = false;
       try {
         await window.__TAURI__.core.invoke('open_url', { url: 'file:///C:/Windows/System32/cmd.exe' });
       } catch {
         rejected = true;
       }

       return JSON.stringify({
         strong: body?.querySelector('strong')?.textContent ?? null,
         code: body?.querySelector('code')?.textContent ?? null,
         href: body?.querySelector('a.slip__link')?.getAttribute('href') ?? null,
         fullText: body?.textContent ?? '',
         timeLabel: time?.textContent ?? '',
         timeAttr: time?.getAttribute('datetime') ?? '',
         rejected,
       });
     })()`,
    true,
  ),
);
check("粗体渲染成 strong", inline.strong === "重点", `strong=${inline.strong}`);
check("行内代码渲染成 code", inline.code === "npm run build", `code=${inline.code}`);
check("链接 href 剥掉了尾部标点", inline.href === "https://example.com/a?x=1&y=2",
  `href=${inline.href}`);
check("被剥掉的标点仍在正文里", (inline.fullText ?? "").includes("。"),
  `text=${inline.fullText}`);
check("open_url 拒绝 file: 协议", inline.rejected === true, `rejected=${inline.rejected}`);
check("新收录的纸条显示『刚刚』", inline.timeLabel === "刚刚", `label=${inline.timeLabel}`);
check("<time> 带 RFC3339 的 datetime", /^\d{4}-\d{2}-\d{2}T/.test(inline.timeAttr ?? ""),
  `datetime=${inline.timeAttr}`);

// 15. 归档抽屉：能翻回来看，也能恢复。
const drawer = JSON.parse(
  await evaluate(
    `(async () => {
       const wait = (ms) => new Promise(r => setTimeout(r, ms));
       const rows = () => [...document.querySelectorAll('.slip')];
       const last = rows()[rows().length - 1];
       const label = last?.querySelector('.slip__text')?.textContent ?? '';

       // 勾选 → 归档这一条
       last.querySelector('.slip__box').click();
       await wait(1200);
       const archiveBtn = rows()[rows().length - 1].querySelector('.slip__archive');
       if (!archiveBtn) return JSON.stringify({ error: '归档按钮没出现' });
       archiveBtn.click();
       await wait(1400);

       const toggle = [...document.querySelectorAll('.paper__archive-all')]
         .find((b) => b.textContent.includes('已归档'));
       const toggleText = toggle?.textContent.trim() ?? '';
       if (!toggle) return JSON.stringify({ error: '没出现已归档入口', toggleText });

       // 进抽屉
       toggle.click();
       await wait(250);
       const inDrawer = rows().length;
       const restoreLabel = rows()[0]?.querySelector('.slip__archive')?.getAttribute('aria-label') ?? '';
       const drawerText = rows()[0]?.querySelector('.slip__text')?.textContent ?? '';

       // 恢复
       rows()[0]?.querySelector('.slip__archive')?.click();
       await wait(1600);
       const backToggle = [...document.querySelectorAll('.paper__archive-all')]
         .filter((b) => b.textContent.includes('已归档')).length;
       const backInList = rows().some((r) => r.querySelector('.slip__text')?.textContent === label);

       return JSON.stringify({ error: null, label, toggleText, inDrawer, restoreLabel, drawerText, backToggle, backInList });
     })()`,
    true,
  ),
);
check("归档后出现『已归档 N』入口",
  /^已归档 \d+$/.test(drawer.toggleText ?? ""), `toggleText=${drawer.toggleText}`);
check("抽屉里能看到归档的纸条", drawer.inDrawer >= 1 && drawer.drawerText === drawer.label,
  `count=${drawer.inDrawer} text=${drawer.drawerText}`);
check("抽屉里的按钮是『恢复纸条』", drawer.restoreLabel === "恢复纸条",
  `aria-label=${drawer.restoreLabel}`);
check("恢复后纸条回到列表且入口消失",
  drawer.backToggle === 0 && drawer.backInList === true,
  `toggle=${drawer.backToggle} backInList=${drawer.backInList}`);

// 16. 导出：只写一个新文件，不动 inbox.md。
const exported = JSON.parse(
  await evaluate(
    `(async () => {
       const wait = (ms) => new Promise(r => setTimeout(r, ms));
       const btn = [...document.querySelectorAll('.paper__archive-all')]
         .find((b) => b.textContent.trim() === '导出');
       if (!btn) return JSON.stringify({ error: '没有导出按钮' });
       btn.click();
       await wait(1300);
       return JSON.stringify({
         error: null,
         hint: document.querySelector('.capture__hint')?.textContent ?? '',
       });
     })()`,
    true,
  ),
);
check("导出后提示区给出文件名",
  !exported.error && /^已导出：export-\d+\.md$/.test(exported.hint ?? ""),
  `hint=${exported.hint}`);

// 17. 统计概览：状态行放人会关心的数，rev 与路径退到悬浮提示。
const stats = JSON.parse(
  await evaluate(
    `JSON.stringify({
       meta: document.querySelector('.paper__meta')?.textContent ?? '',
       title: document.querySelector('.paper__meta')?.getAttribute('title') ?? '',
     })`,
    true,
  ),
);
check("状态行给出未完成与已完成条数",
  /\d+ 未完成/.test(stats.meta ?? "") && /\d+ 已完成/.test(stats.meta ?? ""),
  `meta=${stats.meta}`);
check("rev 收进悬浮提示而不是常驻",
  /rev \d+/.test(stats.title ?? ""), `title=${stats.title}`);

cdp.close();
process.exit(failed ? 1 : 0);
