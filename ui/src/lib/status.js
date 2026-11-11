/**
 * 横幅严重级别：把「快照状态」映射成「视觉强度」。
 *
 * 抽成纯函数而不是写在组件里，是为了能**脱离应用**验证——界面要跑起来才能做 CDP 取证，
 * 而这段判定是纯逻辑，一套 `bun` 断言就够（见 `tools/check-ui-logic.mjs`）。
 *
 * 三档的判据与设计文档 §5 的展示优先级一致，只是换了问法：
 * 优先级问"哪条先显示"，这里问"该多重"。
 */

/**
 * @param {object|null|undefined} snapshot
 * @returns {"error"|"warn"|"info"}
 */
export function bannerLevel(snapshot) {
  if (!snapshot) return "info";
  // Fatal：运行时已停止。内容还在，但功能没了——必须一眼看见。
  if (snapshot.runtime?.state === "fatal") return "error";
  // 数据可能有风险、需要用户处理的三类。
  if (
    snapshot.recovery?.state === "isolated" ||
    (snapshot.health && snapshot.health.writable === false) ||
    snapshot.needsReload === true
  ) {
    return "warn";
  }
  return "info";
}

/**
 * 要不要显示「只读排查指引」。
 *
 * 只在**确实不可写**时出现：能写的时候弹一堆步骤只是噪音，用户会学会无视它。
 * 抽成纯函数是为了能断言这个边界（`writable` 缺失/为真/为假三种）。
 *
 * @param {{health?: {writable?: boolean}}|null|undefined} snapshot
 * @returns {boolean}
 */
export function shouldShowReadonlyGuide(snapshot) {
  return snapshot?.health?.writable === false;
}

/**
 * 热键未注册时，横幅该说什么。
 *
 * 为什么值得单独一条：全局热键被别的程序占用时，应用的**一切看起来都正常**——
 * 进程在、托盘在、窗口能点开，只是按热键没反应。用户会以为是自己记错了键。
 * 所以这条提示必须明确指出"不是你按错，是被占用了"，并给出可操作的下一步。
 *
 * @param {{registered?: boolean, text?: string}|null|undefined} status
 * @returns {string} 空串表示不需要提示
 */
export function hotkeyWarning(status) {
  if (!status || status.registered !== false) {
    return "";
  }
  const name = status.text ? `（${status.text}）` : "";
  return `全局热键${name}未注册，多半已被其它程序占用。可以用托盘图标打开；关掉占用它的程序后重启 Slip 即可恢复。`;
}

/** 回到前台后最少隔多久才静默刷新一次（毫秒）。 */
export const SILENT_REFRESH_GAP_MS = 2000;

/**
 * 拿回焦点时要不要静默拉一次快照。
 *
 * 场景：合上盖子、切走很久、系统休眠——期间磁盘上的 `inbox.md` 可能被别的程序改过，
 * 而应用只知道"上次看到的样子"。回到前台不刷新，用户就会对着过期内容操作，
 * 下一次写入直接撞冲突。
 *
 * 但**不能每次 focus 都刷**：Alt+Tab 来回切、点击窗口、打开菜单都可能连续触发，
 * 那样会把 get_snapshot 打成高频请求。所以设一个最小间隔。
 *
 * @param {number|null|undefined} lastAt 上次刷新时刻（毫秒）
 * @param {number} now 当前时刻（毫秒）
 * @param {number} [gapMs]
 * @returns {boolean}
 */
export function shouldSilentRefresh(
  lastAt,
  now,
  gapMs = SILENT_REFRESH_GAP_MS,
) {
  if (typeof lastAt !== "number" || !Number.isFinite(lastAt)) {
    return true;
  }
  const elapsed = now - lastAt;
  // 时钟回拨（elapsed < 0）按"该刷"处理：宁可多刷一次，也不要拿着可能过期的状态。
  if (elapsed < 0) {
    return true;
  }
  return elapsed >= gapMs;
}
