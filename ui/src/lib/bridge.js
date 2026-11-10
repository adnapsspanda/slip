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

/** 归档一条或多条已完成的纸条。 @param {string} requestId @param {string[]} ids @returns {Promise<Ack>} */
export const archive = (requestId, ids) =>
  call("archive", { requestId, ids });

/** 撤销栈在后端；前端只负责发请求，不维护自己的历史副本。 @returns {Promise<Ack>} */
export const undo = (requestId) => call("undo", { requestId });

/** @returns {Promise<Ack>} */
export const reload = (requestId) => call("reload", { requestId });

/** @returns {Promise<Ack>} */
export const isolateAndReset = (requestId) =>
  call("isolate_and_reset", { requestId });

/** @returns {Promise<string>} */
export const vaultPath = () => call("vault_path");

export const flushPrefs = () => call("flush_prefs");

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
