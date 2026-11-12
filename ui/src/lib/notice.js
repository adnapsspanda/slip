/**
 * 撤销提示的**排队模型**。
 *
 * 问题：连续操作时每一条通知都会覆盖上一条，用户看到"已完成"就以为只有一步可撤，
 * 实际上后面还压着好几步。3 秒内连着做三次，界面却说"1 步"，这是会让人按错的谎报。
 *
 * 做法：只维护一个"连击计数"（`{count, at}`），不进任何业务状态——
 * - 成功一次可撤销操作 → 计数 +1（距上次超过窗口就重新从 1 开始）；
 * - 撤销成功一次 → 计数 -1，减到 0 就清空（提示不再给撤销入口）；
 * - 提示文案 = 最新一条的描述 + "，还有 N 步可撤销"（N 就是计数本身）。
 *
 * 为什么不做真正的通知列表：后端只有**一个**撤销栈，界面上排出一列"待撤销项"会
 * 暗示可以跳过中间某一步去撤销——那件事后端并不支持。计数只回答"还能按几下"。
 *
 * 硬契约：`summon` / `reload` / `isolate` / `restore` 会整体丢弃这个计数（见 App.svelte），
 * 因为这些操作要么换掉了上下文（重新唤出），要么把撤销栈本身清空了。计数留着
 * 就会变成"说还能撤 3 步，实际一步也撤不了"。
 */

/** 连击窗口：三次操作都在 3 秒内才算同一串。 */
export const UNDO_BURST_WINDOW_MS = 3000;

/** @typedef {{count: number, at: number}} UndoTrail */

/**
 * 记一次可撤销操作，返回新的计数。
 *
 * 时钟回拨（gap < 0）按"新一轮"处理：宁可少报一步，也不能把两段不相干的时序粘在一起。
 *
 * @param {UndoTrail|null} trail 上一次的计数
 * @param {number} now 当前毫秒时间
 * @param {number} [windowMs]
 * @returns {UndoTrail}
 */
export function extendUndoBurst(trail, now, windowMs = UNDO_BURST_WINDOW_MS) {
  const at = Number.isFinite(now) ? now : Date.now();
  const valid =
    trail &&
    Number.isFinite(trail.count) &&
    trail.count >= 1 &&
    Number.isFinite(trail.at);
  if (!valid) return { count: 1, at };
  const gap = at - trail.at;
  if (gap < 0 || gap > windowMs) return { count: 1, at };
  return { count: Math.floor(trail.count) + 1, at };
}

/**
 * 撤销成功一次之后，计数该怎么变。
 *
 * 返回 `null` 表示"没有剩余步骤了"：提示条这时不该再挂撤销按钮，
 * 否则用户会按一个必然失败的东西。
 *
 * @param {UndoTrail|null} trail
 * @param {number} now
 * @returns {UndoTrail|null}
 */
export function dropUndoBurst(trail, now) {
  if (!trail || !Number.isFinite(trail.count) || trail.count <= 1) return null;
  return {
    count: Math.floor(trail.count) - 1,
    at: Number.isFinite(now) ? now : trail.at,
  };
}

/**
 * "还有 N 步可撤销"的后缀；只有一步时为空串（一句话就够，不必啰嗦）。
 *
 * @param {number} count
 * @returns {string}
 */
export function undoSuffix(count) {
  return Number.isFinite(count) && count > 1
    ? `，还有 ${Math.floor(count)} 步可撤销`
    : "";
}

/**
 * 组装一条可撤销的通知。
 *
 * 抽出来是为了让"最新一条 + 剩余步数"这句话有唯一出处：文案散在调用点上，
 * 迟早出现"按钮写着撤销、正文却说没有可撤销"的分裂。
 *
 * @param {string} message 最新这一步的描述
 * @param {number} count 当前连击计数
 * @returns {{message: string, undo: boolean}}
 */
export function undoNotice(message, count) {
  return { message: `${message}${undoSuffix(count)}`, undo: true };
}
