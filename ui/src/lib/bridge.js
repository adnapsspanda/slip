/**
 * 前端与后端的唯一接触点。
 *
 * 所有 `invoke` 调用都收敛在这里，原因有两个：
 * 1. 换成别的宿主（浏览器、测试桩）时只改这一个文件；
 * 2. IPC 命令名是契约的一部分，散落在组件里就没人能一眼看全。
 *
 * 类型来自 `crates/slip-app/src/lib.rs` 的 `Serialize` 定义，字段一律 camelCase。
 *
 * @typedef {object} SlipView
 * @property {string} id
 * @property {string} text
 * @property {boolean} done
 * @property {boolean} archived
 * @property {string} createdAt
 * @property {string|null} doneAt
 * @property {boolean} locked
 * @property {boolean} pinned 置顶标志（决定未归档列表的展示顺序，见 lib/order.js）
 *
 * @typedef {object} UiSnapshot
 * @property {number} contentRev
 * @property {number} uiRev
 * @property {SlipView[]} slips
 * @property {{ writable: boolean, warnings: string[] }} health
 * @property {boolean} needsReload
 * @property {{ state: "normal" } | { state: "isolated", corruptPath: string }} recovery
 * @property {string[]} diagnostics
 * @property {{ state: "running" } | { state: "fatal", reason: string }} runtime
 *
 * @typedef {object} Ack
 * @property {string} requestId
 * @property {"applied"|"rejected"|"conflict"|"needsReload"|"unknown"} result
 * @property {number|null} uiRev
 * @property {{ code: string, message: string }|null} error
 */

const raw = globalThis.__TAURI__?.core?.invoke;

/** 在没有宿主的环境里（普通浏览器）为 false，界面据此降级成只读。 */
export const available = Boolean(raw);

/**
 * @param {string} command
 * @param {Record<string, unknown>} [args]
 * @returns {Promise<any>}
 */
async function call(command, args) {
  if (!raw) {
    throw new Error("不在 Tauri 中运行");
  }
  return raw(command, args);
}

export const newRequestId = () =>
  globalThis.crypto?.randomUUID?.() ?? `req-${Date.now()}-${Math.random()}`;

/** @returns {Promise<UiSnapshot>} */
export const getSnapshot = () => call("get_snapshot");

/** @returns {Promise<Ack>} */
export const capture = (requestId, text) =>
  call("capture", { requestId, text });

/** @returns {Promise<Ack>} */
export const complete = (requestId, id, done) =>
  call("complete", { requestId, id, done });

/**
 * 改写纸条正文。
 *
 * 参数名照抄 Rust 端的 snake_case 字段，Tauri 自己负责映射到命令形参；
 * 这里不做任何名字翻译，否则改一次命令签名就要连带改这里。
 * @returns {Promise<Ack>}
 */
export const edit = (requestId, id, text) =>
  call("edit", { requestId, id, text });

/** 锁定只锁正文，勾选与归档仍由后端判断，前端不代为拦截。 @returns {Promise<Ack>} */
export const setLocked = (requestId, id, locked) =>
  call("set_locked", { requestId, id, locked });

/**
 * 置顶（R94/R95）。**不进撤销栈**：后端 `inverse_batch` 对它返回 `None`，
 * 所以界面不要为它显示"撤销"——那会是一个按了就报错的按钮。
 * @returns {Promise<Ack>}
 */
export const setPinned = (requestId, id, pinned) =>
  call("set_pinned", { requestId, id, pinned });

/**
 * 批量完成（R96）。一个批次在撤销栈里是**一步**，撤销一次即整批回退。
 * @param {string[]} ids @returns {Promise<Ack>}
 */
export const completeMany = (requestId, ids, done) =>
  call("complete_many", { requestId, ids, done });

/** 归档一条或多条已完成的纸条。 @param {string} requestId @param {string[]} ids @returns {Promise<Ack>} */
export const archive = (requestId, ids) => call("archive", { requestId, ids });

/**
 * 把一条已归档的纸条恢复回列表。归档是墓碑不是删除，恢复只是清掉 `archived_at`。
 * @returns {Promise<Ack>}
 */
export const unarchive = (requestId, id) =>
  call("unarchive", { requestId, id });

/** 撤销栈在后端；前端只负责发请求，不维护自己的历史副本。 @returns {Promise<Ack>} */
export const undo = (requestId) => call("undo", { requestId });
/** @returns {Promise<Ack>} */
export const reload = (requestId) => call("reload", { requestId });

/** @returns {Promise<Ack>} */
export const isolateAndReset = (requestId) =>
  call("isolate_and_reset", { requestId });

/** @returns {Promise<string>} */
export const vaultPath = () => call("vault_path");

/**
 * vault 里遗留的损坏文件，新的在前。只含文件名，不含绝对路径——
 * 界面不需要绝对路径，少暴露一点也给"定位不带参数"留了余地。
 * @returns {Promise<string[]>}
 */
export const corruptFiles = () => call("corrupt_files");

/**
 * 归档墓碑的容量统计（R102A/R104）。**只读**：只回答"能清理多少"，不删任何东西。
 *
 * 读不到就让界面不显示这一行，不要用 0 冒充——那会和"抽屉里明明有归档"打架。
 * @returns {Promise<{archived: number, archivedBytes: number, totalRecords: number, totalBytes: number, ratio: number}>}
 */
export const vaultCapacity = () => call("vault_capacity");

/**
 * 彻底删除一条已归档记录（R104）。**不可逆**：后端只允许删 `done && archived_at`
 * 的记录，会先写备份；成功后后端已经清空撤销栈，所以界面**不能**给它挂"撤销"。
 * @returns {Promise<Ack>}
 */
export const purgeArchived = (requestId, id) =>
  call("purge_archived", { requestId, id });

/**
 * 损坏证据文件的**只读**残存文本（前 64 KiB，非法 UTF-8 被替换成 U+FFFD）。
 *
 * **安全约束在后端**：只接受 vault 目录下、文件名以 `inbox.md.corrupt-` 开头的路径，
 * 传别的路径会被拒。所以这里必须传**完整路径**（只给文件名不够），
 * 用 `vaultFilePath(vaultDir, name)` 拼——那个拼接是纯函数，有断言覆盖。
 * @param {string} path
 * @returns {Promise<string>}
 */
export const corruptPreview = (path) => call("corrupt_preview", { path });

/**
 * 把 vault 目录与文件名拼成一个路径。
 *
 * 抽成纯函数是因为有一个容易写错的地方：`vault_path` 返回的目录**可能带尾分隔符**，
 * 直接 `dir + "\\" + name` 会拼出 `D:\vault\\inbox.md.corrupt-1`。多数 API 吃得下，
 * 但后端的 `guard_corrupt_path` 会先 `canonicalize` 父目录再与 vault 比对——
 * 多余的分隔符今天恰好还落在同一目录，哪天实现一改就变成"预览凭空打不开"。
 *
 * 另一个坑是分隔符方向：Linux 上 `vault_path` 返回 `/home/u/Slip`，无条件补反斜杠会得到
 * `/home/u/Slip\inbox.md.corrupt-1`——那是**另一个文件名**，后端直接拒绝。所以按目录的
 * 形态选分隔符：有盘符（`C:`）或含反斜杠的按 Windows 处理，其余一律用正斜杠。
 *
 * @param {string} dir vault 目录（`vault_path` 的返回值）
 * @param {string} name 文件名
 * @returns {string}
 */
export function vaultFilePath(dir, name) {
  const base = typeof dir === "string" ? dir.replace(/[\\/]+$/, "") : "";
  const leaf = typeof name === "string" ? name : "";
  // 目录为空时就只给文件名（后端会拒绝，先让它自己报）。
  if (base === "") return leaf;
  // POSIX 绝对路径一定以 `/` 开头；即使文件名里合法地含反斜杠，也不能当成 Windows。
  // Windows 的输入（盘符或 UNC）反过来不会以 `/` 开头。
  const windows =
    !base.startsWith("/") && (/^[A-Za-z]:/.test(base) || base.includes("\\"));
  return windows ? `${base}\\${leaf}` : `${base}/${leaf}`;
}

/**
 * 在资源管理器里选中最新的损坏文件。不传参数：定位目标由 Rust 自己从 vault 里查，
 * 否则等于让 webview 决定 explorer 打开什么。
 * @returns {Promise<void>}
 */
export const revealCorrupt = () => call("reveal_corrupt");

/**
 * 用系统默认浏览器打开链接。**校验在 Rust 侧做**，这里只递原文——
 * "这个 URL 能不能开"的判断放在能被注入的地方等于没做。
 * @param {string} url
 * @returns {Promise<void>}
 */
export const openUrl = (url) => call("open_url", { url });

/**
 * 导出成不带机器标记的 Markdown，返回文件路径（后端会顺便在资源管理器里定位）。
 * @returns {Promise<string>}
 */
export const exportMarkdown = () => call("export_markdown");

/**
 * 复制诊断信息到剪贴板（R08）。返回写入的字符数。
 *
 * 剪贴板由 **Rust 侧**写入，webview 没有剪贴板权限——所以这里只是普通的 IPC 调用。
 * @returns {Promise<number>}
 */
export const copyDiagnostics = () => call("copy_diagnostics");

/**
 * 本次运行的热键状态（`{ text, registered }`）。
 *
 * 单独一条命令而不是塞进快照：`Presenter` 在热键注册之前就已创建、且由消费者任务独占，
 * 为一条"启动即定"的信息去改那条单写者路径不值。
 * @returns {Promise<{text: string, registered: boolean}>}
 */
export const hotkeyStatus = () => call("hotkey_status");

/**
 * 外部修改的条数级差异（`{added, removed, changed}`）。
 * 只读解析磁盘，不改任何状态——只在 `needsReload` 为真时有意义。
 * @returns {Promise<{added: number, removed: number, changed: number}>}
 */
export const externalDiff = () => call("external_diff");

/**
 * 列出 vault 里的备份（新的在前），只给名字/大小/毫秒时间戳。
 * @returns {Promise<Array<{name: string, size: number, stampMs: number}>>}
 */
export const backupList = () => call("backup_list");

/**
 * 用指定备份恢复。名字的合法性由 Rust 侧判定，前端只负责传选中的那个。
 * @returns {Promise<Ack>}
 */
export const restoreBackup = (requestId, name) =>
  call("restore_backup", { requestId, name });

/**
 * 订阅热键唤出事件。非 Tauri 环境返回一个空的退订函数。
 * @param {() => void} handler
 * @returns {Promise<() => void>}
 */
export const subscribeSummon = async (handler) => {
  const listen = globalThis.__TAURI__?.event?.listen;
  if (typeof listen !== "function") {
    return () => {};
  }
  return listen("slip://summon", handler);
};

/** 收起窗口：捕获成功或按 Esc 后调用，属于产品契约。 */
export const dismissWindow = () => call("dismiss_window");
