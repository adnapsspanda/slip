/**
 * 两条会直接影响用户决策的文案：写入失败时草稿还在不在、恢复备份会覆盖多少。
 *
 * 为什么抽出来：这两句话都必须在**错误路径**上出现，而错误路径恰恰是最难肉眼回归的
 * ——要造一个"文件被别的程序占住"或"不可写"的环境才能看见。做成纯函数之后，
 * 文案与判定都能在 `tools/check-ui-logic.mjs` 里断言，不必等真的出事。
 */

/**
 * 写入失败时，能不能把锅说清楚（R107）。
 *
 * 设计前提：本项目**不做任何本地暂存**——草稿本来就只活在捕获框的 `text` 里，
 * 失败路径不清空它（见 `App.svelte` 的 `clearDraft` 只在成功分支被置真）。
 * 所以这里要做的不是"把内容存起来"，而是**明确告诉用户它还在**，
 * 否则用户会以为写失败 = 内容没了，于是去别处重打一遍。
 *
 * @param {{error?: {code?: string, message?: string}, result?: string}|null|undefined} ack
 * @returns {string} 专用文案；空串表示这条错误不该由本函数接管
 */
export function captureFailureHint(ack) {
  const code = ack?.error?.code;
  // `io` 是磁盘层面的失败（被占用、无权限、磁盘满），`busy` 是写入通道暂时排满。
  // 这两种都不是"内容有问题"，而是"现在写不进去"——正是要说清草稿还在的场合。
  if (code !== "io" && code !== "busy") {
    return "";
  }
  const detail = ack?.error?.message;
  const suffix =
    typeof detail === "string" && detail !== "" ? `（${detail}）` : "";
  return `磁盘被占用，内容还在输入框里，处理后可重试${suffix}`;
}

/**
 * 恢复备份前的覆盖警告（R109）。
 *
 * 只说"将覆盖当前内容"是不够的：用户看不出代价有多大。报一个**真实数字**，
 * 他才能判断"要不要先导出"。数字拿不到时明说"无法统计"——
 * 绝不能编一个 0，那会让一个不可逆操作看起来毫无代价。
 *
 * @param {number|null|undefined} pending 当前未归档且未完成的条数
 * @returns {string}
 */
export function restoreCoverWarning(pending) {
  if (typeof pending !== "number" || !Number.isFinite(pending) || pending < 0) {
    return "恢复后将覆盖当前内容（无法统计未完成条数）";
  }
  return `恢复后将覆盖当前 ${Math.floor(pending)} 条未完成项`;
}
