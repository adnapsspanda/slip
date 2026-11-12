<script>
  import { onMount, tick } from "svelte";
  import {
    bannerLevel as statusBannerLevel,
    shouldShowReadonlyGuide,
    hotkeyWarning,
    shouldSilentRefresh,
  } from "./lib/status.js";
  import { noticeAction, filterKeyIntent } from "./lib/keys.js";
  import { matchesFilter, parseQuery } from "./lib/filter.js";
  import { orderSlips } from "./lib/order.js";
  import { parseIso } from "./lib/time.js";
  import { dropUndoBurst, extendUndoBurst, undoNotice } from "./lib/notice.js";
  import {
    captureFailureHint,
    capacitySummary,
    describeDiff,
    purgeNotice,
    restoreCoverWarning,
  } from "./lib/messages.js";
  import { closeFadeDelay } from "./lib/motion.js";
  import {
    createSerialQueue,
    nextToggleTarget,
    shouldDropPendingIntent,
  } from "./lib/queue.js";
  import { confirmNext, CONFIRM_WINDOW_MS } from "./lib/confirm.js";
  import * as bridge from "./lib/bridge.js";
  import Capture from "./lib/Capture.svelte";
  import SlipList from "./lib/SlipList.svelte";

  /** 默认提示文案，唤出和放弃捕获时都要回到它，所以只写一次。 */
  const DEFAULT_HINT = "Enter 收录 · Shift+Enter 换行";

  /** 后端发布的最新快照；`null` 表示还没取到。 */
  let snapshot = $state(null);
  let hint = $state(DEFAULT_HINT);
  let vaultLabel = $state("");
  let captureRef = $state(null);
  /** 列表组件句柄：捕获框里按 ↓ 时要把焦点交给它。 */
  let listRef = $state(null);
  /** 正在等待应答的提交；迟到 ACK 必须与这条不可变记录比较。 */
  let inFlight = $state(null);

  /**
   * 未落盘的勾选意图：`{ [slipId]: boolean }`。
   *
   * 没有它就会出现"连点两下只变一次"——第二次点击读到的快照还是旧的，
   * 于是两次都发同一个目标状态。有了意图记录，第二下是从意图反转。
   */
  let pendingDone = $state({});

  /**
   * 每个 slip 在途/排队的勾选请求数。
   *
   * 只靠 `pendingDone` 无法判断"现在删意图安不安全"：同一 id 可能还有排队点击。
   * 计数归零时才允许丢弃意图，否则第三次点击会从旧快照反转。
   */
  let pendingToggleCounts = $state({});

  /** 热键状态。启动时取一次；唤出/回到前台时重取（托盘换键后横幅要跟着变）。 */
  let hotkeyState = $state(null);

  /** 变更命令串行执行：避免并发请求互相踩，也避免"旧快照"导致的丢失更新。 */
  const mutating = createSerialQueue();
  /** 列表里的编辑态；Ctrl+Z 只在没在编辑时才抢撤销。 */
  let listEditing = $state(false);
  /** 撤销提示：与普通 hint 分开，因为只有它需要挂一个按钮。 */
  let notice = $state(null);
  /** 淡出中：先降到透明，再真正从 DOM 移除，避免"啪一下消失"。 */
  let noticeLeaving = $state(false);
  let noticeTimer = null;
  let noticeFadeTimer = null;

  /**
   * 提示条 4 秒后自动收起。
   *
   * 用 `$effect` 挂在这里而不是在每个赋值点写定时器：`notice` 有 8 处赋值，
   * 逐处维护必然漏。这里只要 notice 变成非空就重排一次计时（所以连续操作会
   * 把倒计时重置，而不是被上一条的定时器提前收走）。
   */
  $effect(() => {
    const current = notice;
    clearTimeout(noticeTimer);
    clearTimeout(noticeFadeTimer);
    if (!current) {
      noticeLeaving = false;
      return;
    }
    noticeLeaving = false;
    noticeTimer = setTimeout(() => {
      noticeLeaving = true;
      noticeFadeTimer = setTimeout(() => {
        notice = null;
        noticeLeaving = false;
      }, 220);
    }, 4000);
    return () => {
      clearTimeout(noticeTimer);
      clearTimeout(noticeFadeTimer);
    };
  });
  /** 撤销请求在途，避免连按 Ctrl+Z 把后端的撤销栈一次抽干。 */
  let undoing = $state(false);
  /**
   * vault 里遗留的损坏文件（`inbox.md.corrupt-*`）。
   *
   * 这是启动时的一次性事实，不需要订阅；取不到就当作没有——它是提示，不是门禁，
   * 不该因为读目录失败就在界面上报警。
   */
  let corrupt = $state([]);

  /**
   * vault 目录**全路径**（不只是末段名字）。R108 的损坏预览要求传"目录 + 文件名"，
   * 后端只会放行 vault 目录下 `inbox.md.corrupt-*` 的文件。
   */
  let vaultDir = $state("");

  /**
   * 损坏文件的只读预览：`{ name, text, error, loading }`，`null` = 没打开。
   *
   * 设计上没有任何"保存/写回"入口：这是证据，不是可编辑内容。能做的事只有看和复制。
   */
  let preview = $state(null);
  /** `<pre>` 元素，供"全选"用。 */
  let previewEl = $state(null);
  /** 弹窗面板本身：打开时接住焦点，让 Tab 链不跑到弹窗背后的控件上。 */
  let previewPanelEl = $state(null);

  /** 收起窗口时的容器淡出标记（R119）。 */
  let leaving = $state(false);

  // 归档判定只看 `archived`：后端 `SlipView.archived` 是唯一判据
  // （`done && archived_at.is_some()`，见 v11 §1.8），没有独立的 archivedAt 字段。
  let activeSlips = $derived(
    (snapshot?.slips ?? []).filter((slip) => !slip.archived),
  );
  let archivedSlips = $derived(
    (snapshot?.slips ?? []).filter((slip) => slip.archived),
  );
  /**
   * 未归档列表的**展示顺序**（R94）：置顶的稳定浮到最前，组内仍是创建顺序。
   *
   * 排序只发生在这里——`activeSlips` 保持快照原序，撤销、归档批处理、键盘导航
   * 用的都是"当前该看到的那个顺序"，两者不能各自排一遍（否则 `itemEls[index]`
   * 会和快照下标错位）。
   */
  let orderedActive = $derived(orderSlips(activeSlips));
  /** 是否在看归档抽屉。归档是墓碑不是删除，所以这一层只是换个筛选方向。 */
  let showArchived = $state(false);
  /** 备份面板是否展开。 */
  let showBackups = $state(false);
  /** 备份列表；只在打开面板时取一次。 */
  let backups = $state([]);
  /**
   * 备份列表请求的代数。
   *
   * 每次开/合面板都自增；响应回来时只有代数仍是最新、且面板还开着才允许落盘。
   * 否则「快速开→关→开」时旧响应会覆盖新结果，面板关掉后失败还会把 hint 改成
   * 一句没人看得懂的"读取备份失败"。
   */
  let backupsToken = 0;
  /**
   * 归档抽屉的容量摘要（R104）。`vault_capacity` 是只读统计，本地算不出来（要读盘）。
   * 读不到就保持 null → `capacityLine` 为空串 → 整行不显示，绝不用 0 冒充。
   */
  let capacity = $state(null);
  let capacityLine = $derived(capacitySummary(capacity));
  /** 等待二次确认的备份名——恢复会覆盖当前内容，不该一次点击就执行。 */
  let pendingRestore = $state(null);
  /**
   * "会改数据"的二次确认共用一套武装状态：隔离重建、归档已完成（R97）、
   * 完成匹配项（R96）、彻底删除归档项（R104，键是 `purge:<id>`）。`null` = 未武装。
   *
   * 为什么合成一个对象 + 一个入口：这些操作的窗口语义完全相同（4 秒自动还原）。
   * 抄多份的下场是"改了窗口只改到两处"，而这类不一致只在手滑误触时才暴露出来。
   * 彻底删除需要"哪一条被武装"，所以它的键带 id；未武装项从对象的键上自然也读得出来。
   */
  let armed = $state({ isolate: null, archive: null, complete: null });
  /** 每个操作各一个自动还原定时器。 */
  const armTimers = {};

  /** 归档抽屉里"已被武装待彻底删除"的那一条 id（没有则是 null）。 */
  let purgeArmedId = $derived.by(() => {
    for (const key of Object.keys(armed)) {
      if (key.startsWith("purge:") && armed[key] !== null) return key.slice(6);
    }
    return null;
  });

  /**
   * 二次确认闸门：第一下武装（并起 4 秒定时器），窗口内第二下返回 true 表示执行。
   *
   * 状态迁移本身在 `confirm.js` 的 `confirmNext` 里（纯函数，可断言"超时自动还原"）；
   * 这里只负责两件有副作用的事：记/清定时器、把时间戳写回响应式状态。
   *
   * @param {string} kind
   * @returns {boolean}
   */
  function confirmGate(kind) {
    const next = confirmNext(armed[kind], Date.now());
    clearTimeout(armTimers[kind]);
    armed[kind] = next.armedAt;
    if (next.execute) return true;
    armTimers[kind] = setTimeout(() => {
      armed[kind] = null;
    }, CONFIRM_WINDOW_MS);
    return false;
  }

  /**
   * 撤销连击计数（R100）：`{count, at}`。
   *
   * 不进快照、不落盘——它是**提示区的排队模型**，只用来回答"现在还能按几下 Ctrl+Z"。
   * 后端只有一个撤销栈，界面不能假装能跳过中间某一步（见 lib/notice.js 的说明）。
   */
  let undoTrail = $state(null);

  /**
   * 过滤关键字。**纯视图**：只影响显示，不影响快照、撤销栈与磁盘。
   *
   * 撤销按后端真实提交顺序记录；过滤若参与排序或索引，`Ctrl+Z` 就会撤错东西。
   */
  let filter = $state("");
  /** 过滤条是否展开。Ctrl+F 展开，Esc 收起。 */
  let filtering = $state(false);
  let filterRef = $state(null);

  /**
   * 过滤条件只解析一次，然后逐条判定。
   *
   * 判定逻辑全部在 `lib/filter.js` 的纯函数里（`#tag`、`is:done`/`is:open`/`is:locked`，
   * 其余为全文模糊搜索；不合法的 `is:xxx` 降级而不报错、不清空列表）。
   * 这里只负责"取哪一侧 + 交给谁"，因为这两件事依赖界面状态，不该进纯函数。
   */
  let parsedFilter = $derived(parseQuery(filter));
  let visible = $derived.by(() => {
    const base = showArchived ? archivedSlips : orderedActive;
    if (!parsedFilter.constrained) return base;
    return base.filter((slip) => matchesFilter(slip, parsedFilter));
  });
  /**
   * 过滤结果里**还没完成**的那些——「完成匹配项」只对这些发请求。
   *
   * 已完成的条目再"完成"一次是无操作；把它们塞进批次里会让用户看到
   * "已完成 5 条"但其实只有 2 条变了，也让撤销步变得含混。
   */
  let matchPending = $derived(visible.filter((slip) => !slip.done));
  // 统计始终看**未归档**那一侧：归档抽屉里显示"3 条未完成"只会让人困惑。
  let pendingCount = $derived(activeSlips.filter((slip) => !slip.done).length);
  let doneSlips = $derived(activeSlips.filter((slip) => slip.done));
  let doneCount = $derived(doneSlips.length);

  /**
   * 今日新增条数（本地日界）。
   *
   * 跟相对时间一样不轮询：跨过零点时数字会短暂偏大，下次快照更新就对了——
   * 为这个挂定时器不值得。
   */
  let todayCount = $derived.by(() => {
    const start = new Date();
    start.setHours(0, 0, 0, 0);
    const since = start.getTime();
    return activeSlips.filter((slip) => {
      const at = parseIso(slip.createdAt);
      return !Number.isNaN(at) && at >= since;
    }).length;
  });

  /**
   * 状态行只放**人会关心的数**。
   *
   * `rev` 与 vault 路径移到悬浮提示：前者是排查用的，后者太长，都不该占常驻位置。
   */
  let meta = $derived(
    [
      `${pendingCount} 未完成`,
      `${doneCount} 已完成`,
      archivedSlips.length > 0 ? `${archivedSlips.length} 已归档` : "",
      todayCount > 0 ? `今日 +${todayCount}` : "",
      corrupt.length > 0 ? `${corrupt.length} 个损坏文件` : "",
    ]
      .filter(Boolean)
      .join(" · "),
  );
  let metaTitle = $derived(
    [vaultLabel, snapshot ? `rev ${snapshot.contentRev}` : ""]
      .filter(Boolean)
      .join(" · "),
  );

  /**
   * 外部修改的条数级差异。
   *
   * 只在 `needsReload` 为真时取一次——那是"磁盘比内存新"的唯一信号。
   * 不轮询：外部修改不是每秒发生的事。
   */
  let externalDiff = $state(null);

  $effect(() => {
    if (!snapshot?.needsReload) {
      externalDiff = null;
      return;
    }
    let cancelled = false;
    bridge
      .externalDiff()
      .then((summary) => {
        if (!cancelled) externalDiff = summary;
      })
      .catch(() => {
        // 读不到就不显示条数：横幅本身已经说了"文件被外部修改"。
        if (!cancelled) externalDiff = null;
      });
    return () => {
      cancelled = true;
    };
  });

  let banner = $derived.by(() => {
    if (!snapshot) {
      return "";
    }
    // §5 展示优先级：Fatal > Isolated > Degraded > needs_reload，只显示最需要处理的一条。
    if (snapshot.runtime?.state === "fatal") {
      return `运行时已停止：${snapshot.runtime.reason}`;
    }
    if (snapshot.recovery?.state === "isolated") {
      return `无效文件已隔离到 ${snapshot.recovery.corruptPath}。可以重建一个空 Inbox。`;
    }
    if (snapshot.health && !snapshot.health.writable) {
      // 不可写的原因不止"解析失败"：文件被其它程序占用、读盘失败同样会让它变 false。
      // 具体原因在 diagnostics 里（例如"文件被其它程序占用"），这里只说清楚该怎么办。
      return "当前不可写入。处理好下方诊断里的问题后可重新载入，或隔离并重建空 Inbox。";
    }
    if (snapshot.needsReload) {
      return externalDiff
        ? `磁盘上的文件已被外部修改（${describeDiff(externalDiff)}），需要重新载入。`
        : "磁盘上的文件已被外部修改，需要重新载入。";
    }
    // 优先级最低：内容层面的问题更急；但只要没有更急的，热键问题就必须让用户看见
    // ——否则表现是"按热键没反应"，用户只会以为是自己记错了键。
    return hotkeyWarning(hotkeyState);
  });

  // 判据抽在 lib/status.js：纯函数，能脱离应用验证（tools/check-ui-logic.mjs）。
  let bannerLevel = $derived.by(() => {
    const level = statusBannerLevel(snapshot);
    // 热键未注册不是内容危险，但确实"需要你处理"，归到 warn；
    // 已经在 error（Fatal）时不能降级。
    if (level === "info" && hotkeyWarning(hotkeyState) !== "") {
      return "warn";
    }
    return level;
  });

  let diagnostics = $derived(snapshot?.diagnostics ?? []);

  /** 把后端内部错误码转换为面向用户的中文行动提示。 */
  function formatAckError(ack) {
    const code = ack?.error?.code;
    const msg = ack?.error?.message ?? ack?.result ?? "未知错误";
    switch (code) {
      case "conflict":
      case "needsReload":
        return "磁盘上的文件已被修改，请先点上方「重新载入」";
      case "readOnly":
        return "文件无法解析，当前处于只读保护状态";
      case "invalidBackup":
        // 关键信息是"当前文件没动"：用户会以为恢复失败等于数据更糟了，
        // 其实只是这一份备份本身坏了，选另一代即可。
        return "这份备份内容无法解析，未恢复；当前 inbox.md 未被改动，可换一代备份";
      case "busy":
        return "操作过于频繁，请稍后再试";
      case "notExecuted":
        return "后台未执行该操作，请重试";
      case "dropped":
        return "后台任务已退出，未收到应答；请重启 Slip";
      case "fatal":
        return "运行时状态异常，请重启 Slip";
      case "undo":
        return msg;
      case "blocking":
        // 后端的 message 已经是人类可读的句子（BlockingError 实现了 Display），
        // 这里只补一句"该怎么办"。
        return `inbox.md 有无法处理的结构问题：${msg}。修好文件后重新载入`;
      case "io":
        // `IoKind` 的 Display 已经是中文（文件不存在/权限不足/被占用…），
        // 旧实现还在剥一个早已不存在的 "i/o error:" 前缀，纯属历史残留。
        return `磁盘读写失败（${msg}）`;
      case "notDegraded":
        return "当前文件状态正常，不需要隔离";
      case "notArchived":
        return "只有已归档的纸条才能彻底删除";
      case "domain":
        // 领域层的 Display 早已全部中文化，并且就是给用户看的句子；
        // 这里不要再按英文子串猜含义（那三条分支早已是死代码）。
        return msg;
      default:
        return msg;
    }
  }

  /** 内容提交被禁用时的原因；空字符串表示可以提交。 */
  let blockedReason = $derived.by(() => {
    if (!bridge.available) return "后端不可用";
    if (snapshot?.runtime?.state === "fatal") return "运行时已停止，不能收录";
    if (snapshot?.recovery?.state === "isolated") return "等待重建 Inbox";
    if (snapshot && !snapshot.health.writable)
      return "当前不可写入，详见横幅诊断";
    if (snapshot?.needsReload) return "磁盘文件已变化，请先重新载入";
    return "";
  });

  /**
   * 重新拉取快照。
   *
   * 只在 `ui_rev` 不比手上旧时替换——这是"发射丢失"能被补偿的唯一机制，
   * 也是前端唯一被允许做的版本判断。
   */
  /** 上次静默刷新的时刻（毫秒）。用来给 focus 触发的刷新限流。 */
  let lastSilentRefreshAt = $state(null);

  /**
   * 静默刷新：拿回焦点时拉一次最新快照。
   *
   * **不碰提示区**：用户可能只是切了个窗口，不该看到"正在刷新…"这种噪音；
   * 失败也不吭声——下次真正操作时该报的错自然会报。
   */
  async function silentRefresh() {
    const now = Date.now();
    if (!shouldSilentRefresh(lastSilentRefreshAt, now)) {
      return;
    }
    lastSilentRefreshAt = now;
    try {
      const next = await bridge.getSnapshot();
      if (!snapshot || next.uiRev >= snapshot.uiRev) {
        snapshot = next;
      }
    } catch {
      // 静默路径不报错：拿不到就沿用当前状态，等真实操作时再说。
    }
  }

  async function refresh() {
    try {
      const next = await bridge.getSnapshot();
      if (!snapshot || next.uiRev >= snapshot.uiRev) {
        snapshot = next;
      }
      return next;
    } catch (error) {
      hint = `读取快照失败：${error}`;
      return null;
    }
  }

  /**
   * 收起窗口。
   *
   * `dismiss_window` 没有进 bridge 的契约层，只能用透传入口调用。收起是收尾
   * 动作：失败最多是窗口留在原地，不值得把错误糊到界面上打断用户。
   */
  /**
   * 收起窗口。
   *
   * 真正的显示/隐藏由 shell 的 `dismiss_window` 决定，前端只能让**容器**先淡出
   * （R119，0.12 秒），再让窗口收起——这是"内容先淡、窗口后收"的组合，
   * 不是原生窗口动画。`prefers-reduced-motion: reduce` 时延迟为 0（立即消失），
   * 理由见 `lib/motion.js`：只把 CSS 时长压到 0 仍会白等 120ms。
   *
   * 失败最多是窗口留在原地，不值得把错误糊到界面上打断用户；但要**把透明度恢复**，
   * 否则界面会停在全透明状态里，看起来像崩了。
   */
  function dismissWindow() {
    const reduced =
      globalThis.matchMedia?.("(prefers-reduced-motion: reduce)")?.matches ===
      true;
    const delay = closeFadeDelay(reduced);
    if (delay === 0) {
      bridge.dismissWindow().catch(() => {});
      return;
    }
    leaving = true;
    setTimeout(() => {
      bridge.dismissWindow().catch(() => {
        leaving = false;
      });
    }, delay);
  }

  /**
   * 热键唤出：保留上次没提交的草稿，只把光标放回去。
   *
   * 失焦自动收走是"暂时离开"，不是"放弃输入"；只有 Esc 才清空草稿。
   */
  function summon() {
    hint = DEFAULT_HINT;
    // 上一次收起可能把容器留在了淡出态（R119）：唤出必须从可见状态开始。
    leaving = false;
    // 撤销提示跨过"收起再唤出"就过期了：那次的修改上下文已经不在了。
    // **连击计数也要一起清**（R100 的既有清栈契约）：唤出之后用户看到的第一条
    // 提示必须是干净的，不能上来就说"还有 2 步可撤销"。
    notice = null;
    undoTrail = null;
    // 视图状态也不该跨过这一次唤出：过滤词、归档抽屉、备份面板都是**当次浏览**的临时
    // 状态。留着它们，下一次唤出会直接落在一张被筛过的列表上——看上去像"东西没了"。
    // 实测过：开着过滤收起窗口，再唤出只看到 1 条，而实际有 2 条。
    filter = "";
    filtering = false;
    showArchived = false;
    showBackups = false;
    pendingRestore = null;
    // 只读预览、备份列表与二次确认武装态同样是"当次浏览"的临时状态。
    // `armed` 尤其危险：武装彻底删除后窗口失焦收起，4 秒内再唤出，
    // 同一行仍是武装态——下一次单击就会真的执行不可逆删除。
    preview = null;
    backups = [];
    for (const key of Object.keys(armed)) armed[key] = null;
    for (const key of Object.keys(armTimers)) {
      clearTimeout(armTimers[key]);
      delete armTimers[key];
    }
    // 未落盘的勾选意图只属于当次会话：上一轮失败/迟到的意图留到下一次唤出，
    // 会让第一下点击从旧意图反转，看上去像勾选失灵。
    pendingDone = {};
    pendingToggleCounts = {};
    void refreshHotkeyStatus();
    captureRef?.focus();
  }

  /** Escape：收走等于放弃这次捕获，残稿不能留到下次唤出。 */
  function abandon() {
    captureRef?.clear();
    hint = DEFAULT_HINT;
    notice = null;
    // 放弃这次捕获 = 上下文结束，撤销连击计数同样作废。
    undoTrail = null;
    dismissWindow();
  }

  async function handleCapture(text) {
    if (inFlight) {
      hint = "上一条还在收录，请稍候…";
      captureRef?.focus();
      return;
    }

    const request = {
      requestId: bridge.newRequestId(),
      draftVersion: captureRef?.getDraftVersion?.() ?? 0,
    };
    inFlight = request;
    let clearDraft = false;

    try {
      const ack = await bridge.capture(request.requestId, text);
      if (ack.requestId !== request.requestId) {
        hint = "收到不匹配的应答，草稿已保留。";
      } else if (ack.result !== "applied") {
        await refresh();
        /*
         * R107：写入失败时要说清"草稿没丢"。
         *
         * 本项目的草稿**只**活在捕获框里（不做任何本地暂存，那会造出第二真源），
         * 而失败路径**从不**置 `clearDraft`——下面这句文案就是把这件事讲出来，
         * 否则用户会把"写不进去"理解成"内容没了"，转身去别处重打一遍。
         */
        hint = captureFailureHint(ack) || `未收录：${formatAckError(ack)}`;
      } else if (ack.uiRev == null) {
        hint = "已收录，但后端未返回界面版本；草稿已保留，可重试同步。";
      } else if (captureRef?.getDraftVersion?.() !== request.draftVersion) {
        hint = "已收录；等待期间你又输入了内容，草稿已保留。";
      } else {
        // 第四条件：先补偿快照，本地真正看见 ack.uiRev 才允许清空。
        const next = await refresh();
        if (!next) {
          hint = "已收录，但界面同步失败；草稿已保留，可再次同步。";
        } else if ((snapshot?.uiRev ?? -1) >= ack.uiRev) {
          captureRef?.clear();
          hint = "已收录";
          clearDraft = true;
        } else {
          hint = "已收录，但本地界面尚未刷新；草稿已保留。";
        }
      }
    } catch (error) {
      // 异常路径同样不清草稿；文案里明说，免得用户以为内容没了。
      hint = `capture 失败（内容还在输入框里）：${error}`;
    } finally {
      inFlight = null;
    }

    if (clearDraft) {
      dismissWindow();
    } else {
      captureRef?.focus();
    }
  }

  /**
   * 记一条"可撤销"的通知（R100）。
   *
   * 所有**进了后端撤销栈**的操作都必须走这里，而不是各自 `notice = {...}`：
   * 连击计数要逐次累加，散在五处赋值点上必然有人漏掉，漏掉的那一处就会在
   * 连续操作时谎报"只有 1 步可撤销"。
   *
   * 反例（不进这里的）：锁定、置顶——后端 `inverse_batch` 对它们返回 `None`，
   * 给它们挂撤销按钮等于给用户一个按了只会在提示区报错的按钮。
   */
  function markUndone(message) {
    undoTrail = extendUndoBurst(undoTrail, Date.now());
    notice = undoNotice(message, undoTrail.count);
  }

  async function handleToggle(slip) {
    // 以"未落盘的意图"为准，而不是当前快照：连点两下时快照还没跟上。
    const targetDone = nextToggleTarget(slip.done, pendingDone[slip.id]);
    pendingDone[slip.id] = targetDone;
    pendingToggleCounts[slip.id] = (pendingToggleCounts[slip.id] ?? 0) + 1;
    let accepted = false;
    try {
      // 串行发送：后一次点击一定排在前一次之后，不会两个请求同时基于同一份旧快照。
      const ack = await mutating.run(() =>
        bridge.complete(bridge.newRequestId(), slip.id, targetDone),
      );
      accepted = ack.result === "applied";
      await refresh();
      if (accepted) {
        if (ack.changed !== false) {
          hint = DEFAULT_HINT;
          markUndone(targetDone ? "已完成" : "已标为未完成");
        } else {
          hint = "没有变化";
        }
      } else {
        hint = `未更新：${formatAckError(ack)}`;
      }
    } catch (error) {
      hint = `complete 失败：${error}`;
      await refresh();
    } finally {
      const remaining = (pendingToggleCounts[slip.id] ?? 1) - 1;
      if (remaining > 0) pendingToggleCounts[slip.id] = remaining;
      else delete pendingToggleCounts[slip.id];
      const stored = snapshot?.slips.find((item) => item.id === slip.id);
      if (
        shouldDropPendingIntent(remaining, accepted, stored?.done, targetDone)
      ) {
        delete pendingDone[slip.id];
      }
    }
  }

  /**
   * 提交正文编辑。
   *
   * 返回布尔值而不是只改 hint：列表要靠它决定要不要关掉编辑框。
   * 失败时返回 false，让用户刚打的字留在输入框里。
   */
  async function handleEdit(slip, text) {
    try {
      const ack = await mutating.run(() =>
        bridge.edit(bridge.newRequestId(), slip.id, text),
      );
      await refresh();
      if (ack.result === "applied") {
        if (ack.changed !== false) {
          hint = DEFAULT_HINT;
          markUndone("已修改");
        } else {
          hint = "没有变化";
        }
        return true;
      }
      hint = `未修改：${formatAckError(ack)}`;
      return false;
    } catch (error) {
      hint = `edit 失败：${error}`;
      return false;
    }
  }

  /**
   * 切换锁定。
   *
   * 无论成败都刷新：锁的显示必须以后端为准，否则界面会替用户"以为"成功了。
   */
  async function handleLock(slip) {
    try {
      const ack = await mutating.run(() =>
        bridge.setLocked(bridge.newRequestId(), slip.id, !slip.locked),
      );
      if (ack.result === "applied") {
        // 锁定不进撤销栈，清掉可能残留的旧撤销条以免误导。
        notice = null;
      } else {
        hint = `锁定状态未变：${formatAckError(ack)}`;
      }
    } catch (error) {
      hint = `set_locked 失败：${error}`;
    }
    await refresh();
  }

  /** 归档单条已完成纸条，并提供一键撤销入口。 */
  async function handleArchive(slip) {
    try {
      const ack = await mutating.run(() =>
        bridge.archive(bridge.newRequestId(), [slip.id]),
      );
      await refresh();
      if (ack.result === "applied") {
        if (ack.changed !== false) {
          hint = DEFAULT_HINT;
          markUndone("已归档");
        } else {
          hint = "没有变化";
        }
      } else {
        hint = `未归档：${formatAckError(ack)}`;
      }
    } catch (error) {
      hint = `archive 失败：${error}`;
      await refresh();
    }
  }

  /**
   * 触控右滑完成（R114）。已完成的条目再右滑不该被"反完成"——
   * 用户滑一下只是想把这条划掉，不是想把它退回待办。
   */
  function handleSwipeComplete(slip) {
    if (slip.done) return;
    handleToggle(slip);
  }

  /** 打开/收起备份面板；打开时现取一遍列表。 */
  async function toggleBackups() {
    // 先记下本次请求的代数：关闭面板或再次打开都会让旧响应作废。
    const token = ++backupsToken;
    showBackups = !showBackups;
    pendingRestore = null;
    if (showBackups) {
      try {
        const list = await bridge.backupList();
        if (token === backupsToken && showBackups) backups = list;
      } catch (error) {
        if (token === backupsToken && showBackups) {
          backups = [];
          hint = `读取备份失败：${error}`;
        }
      }
    }
  }

  /** 两步确认的第二下：真正恢复。 */
  async function handleRestore(backup) {
    if (pendingRestore !== backup.name) {
      pendingRestore = backup.name;
      return;
    }
    pendingRestore = null;
    try {
      const ack = await mutating.run(() =>
        bridge.restoreBackup(bridge.newRequestId(), backup.name),
      );
      await refresh();
      if (ack.result === "applied") {
        // 整份文件换了一个世界：面板与归档抽屉都基于旧内容，必须退出。
        showBackups = false;
        showArchived = false;
        preview = null;
        hint = "已从备份恢复（恢复前的版本也已留档）";
        // 恢复整份文件 = 换了一个世界：撤销栈被后端清空，本地连击计数也必须清。
        notice = null;
        undoTrail = null;
      } else {
        hint = `恢复失败：${formatAckError(ack)}`;
      }
    } catch (error) {
      hint = `恢复失败：${error}`;
    }
  }

  /** 备份时间与大小的展示格式。 */
  function formatStamp(ms) {
    return new Date(ms).toLocaleString("zh-CN", { hour12: false });
  }

  function formatSize(bytes) {
    return bytes >= 1024 ? `${(bytes / 1024).toFixed(1)} KB` : `${bytes} B`;
  }

  /** 重新读取热键状态。托盘换键后，窗口内的横幅要跟着变。 */
  async function refreshHotkeyStatus() {
    try {
      hotkeyState = await bridge.hotkeyStatus();
    } catch {
      // 查询失败不吓用户：横幅不显示比显示一条假报警好。
    }
  }

  /** 复制诊断到剪贴板。纯读取快照，不改任何状态。 */
  async function handleCopyDiagnostics() {
    try {
      const chars = await bridge.copyDiagnostics();
      hint = `诊断已复制（${chars} 字）`;
    } catch (error) {
      hint = `复制失败：${error}`;
    }
  }

  /** 导出为可读 Markdown。不做确认弹窗：它只写一个新文件，不动 inbox.md。 */
  async function handleExport() {
    try {
      const path = await bridge.exportMarkdown();
      hint = `已导出：${path.split(/[\\/]/).pop()}`;
    } catch (error) {
      hint = `导出失败：${error}`;
    }
  }

  /** 把一条已归档的纸条恢复回列表。归档是墓碑不是删除，恢复只是清掉归档时间。 */
  async function handleUnarchive(slip) {
    try {
      const ack = await mutating.run(() =>
        bridge.unarchive(bridge.newRequestId(), slip.id),
      );
      await refresh();
      if (ack.result === "applied") {
        if (ack.changed !== false) {
          hint = "已恢复到列表";
          markUndone("已恢复");
        } else {
          hint = "没有变化";
        }
        // 恢复掉最后一条后抽屉就空了：继续留在空列表里只会让人以为数据没了。
        // （探针实测过这个手感问题。）
        if (archivedSlips.length === 0) showArchived = false;
      } else {
        hint = `恢复失败：${formatAckError(ack)}`;
      }
    } catch (error) {
      hint = `恢复失败：${error}`;
      await refresh();
    }
  }

  /**
   * 打开/收起归档抽屉（R104）。
   *
   * 打开时取一次容量统计：它是"还能清理多少"的唯一来源（归档字节数只能读盘算）。
   */
  function toggleArchived() {
    showArchived = !showArchived;
    if (showArchived) void refreshCapacity();
  }

  /** 取一次容量统计；读不到就清成 null，让摘要行整行消失。 */
  async function refreshCapacity() {
    try {
      capacity = await bridge.vaultCapacity();
    } catch {
      // 读不到就不显示：显示"归档 0 条"会和抽屉里明明存在的记录直接打架。
      capacity = null;
    }
  }

  /** 彻底删除一条归档项的两步确认入口（R104）：武装逻辑见 `confirmGate`。 */
  function handlePurgeClick(slip) {
    // 同时只允许**一条**处于"待确认"：否则武装 A 之后再去点 B，高亮会指在 A 上，
    // 用户按第二下时根本不知道自己确认的是哪一条。清掉别的键就够了——它们的
    // 定时器到点会再写一次 null，无害。
    for (const key of Object.keys(armed)) {
      if (key.startsWith("purge:") && key !== `purge:${slip.id}`)
        armed[key] = null;
    }
    if (confirmGate(`purge:${slip.id}`)) handlePurge(slip);
  }

  /**
   * 彻底删除（R104）。**不可逆**，后端在成功时已清空撤销栈。
   *
   * 所以这里刻意不用 `markUndone`：给一个"撤销"按钮只会让用户按到一个必然报错的
   * 东西。本地连击计数也一并作废——它描述的那个栈已经不存在了。
   */
  async function handlePurge(slip) {
    try {
      const ack = await mutating.run(() =>
        bridge.purgeArchived(bridge.newRequestId(), slip.id),
      );
      // 列表以后端为准：刷新快照，那条自然从抽屉里消失（不做本地乐观删除）。
      await refresh();
      if (ack.result === "applied") {
        hint = DEFAULT_HINT;
        notice = purgeNotice();
        undoTrail = null;
        void refreshCapacity();
        // 删掉最后一条后抽屉就空了：留在空抽屉里只会让人以为数据没了，
        // 而且"返回列表"的入口（有归档才显示）也跟着消失了。
        if (archivedSlips.length === 0) showArchived = false;
      } else {
        hint = `未删除：${formatAckError(ack)}`;
      }
    } catch (error) {
      hint = `purge_archived 失败：${error}`;
      await refresh();
    }
  }

  /**
   * 「归档已完成 (N)」的两步确认入口（R97）：武装逻辑见 `confirmGate`。
   */
  function handleArchiveAllClick() {
    if (confirmGate("archive")) handleArchiveAll();
  }

  async function handleArchiveAll() {
    const ids = doneSlips.map((slip) => slip.id);
    if (ids.length === 0) return;
    try {
      const ack = await mutating.run(() =>
        bridge.archive(bridge.newRequestId(), ids),
      );
      await refresh();
      if (ack.result === "applied") {
        if (ack.changed !== false) {
          hint = DEFAULT_HINT;
          // 一整批在撤销栈里是一步：这里报的是"已归档 N 条"，撤销一次就整批回来。
          markUndone(`已归档 ${ids.length} 条`);
        } else {
          hint = "没有变化";
        }
      } else {
        hint = `未归档：${formatAckError(ack)}`;
      }
    } catch (error) {
      hint = `archive 失败：${error}`;
      await refresh();
    }
  }

  /**
   * 「完成匹配项 (N)」的两步确认入口（R96）：与归档同一套时间语义，但目标是
   * **过滤结果**，所以只有真的筛出未完成项时才该出现这个按钮。
   */
  function handleCompleteManyClick() {
    if (confirmGate("complete")) handleCompleteMany();
  }

  async function handleCompleteMany() {
    const ids = matchPending.map((slip) => slip.id);
    if (ids.length === 0) return;
    try {
      // 串行队列：批量完成与单条勾选共用一条发送链，避免两份快照互相覆盖。
      const ack = await mutating.run(() =>
        bridge.completeMany(bridge.newRequestId(), ids, true),
      );
      await refresh();
      if (ack.result === "applied") {
        if (ack.changed !== false) {
          hint = DEFAULT_HINT;
          markUndone(`已完成 ${ids.length} 条`);
        } else {
          hint = "没有变化";
        }
      } else {
        hint = `未完成：${formatAckError(ack)}`;
      }
    } catch (error) {
      hint = `complete_many 失败：${error}`;
      await refresh();
    }
  }

  /**
   * 切换置顶（R95）。
   *
   * **不进撤销栈**（后端 `inverse_batch` 返回 `None`），所以这里刻意不碰
   * `markUndone`，也不留"撤销"按钮；已有的撤销提示一并收起，免得用户以为
   * 那一步能撤销这次置顶。连击计数保留：之前那些步骤在后端仍然可撤。
   */
  async function handlePin(slip) {
    const target = !slip.pinned;
    try {
      const ack = await mutating.run(() =>
        bridge.setPinned(bridge.newRequestId(), slip.id, target),
      );
      if (ack.result === "applied") {
        hint = target ? "已置顶到最前" : "已取消置顶";
        notice = null;
      } else {
        hint = `置顶未变：${formatAckError(ack)}`;
      }
    } catch (error) {
      hint = `set_pinned 失败：${error}`;
    }
    await refresh();
  }

  /**
   * 恢复被 Ctrl+Backspace 清掉的草稿（R98，单次）。
   *
   * 只在捕获框仍然为空时恢复：用户可能已经重新打了字，直接覆盖等于把新内容
   * 一起吞掉——那比不恢复更糟。恢复后提示条立刻消失（一次性入口）。
   */
  function restoreDraft() {
    const pending = notice?.restore;
    notice = null;
    if (typeof pending !== "string" || pending === "") return;
    if ((captureRef?.getDraft?.() ?? "") !== "") {
      hint = "草稿框里已有内容，没有覆盖";
      return;
    }
    captureRef?.setDraft?.(pending);
    hint = "草稿已恢复";
  }

  /**
   * 撤销上一次改动。栈在后端，这里不做任何本地回滚。
   *
   * 同一时刻只放一个请求进去：撤销栈是后进先出的，并发请求会让用户
   * 多按的那一下撤掉一条并不是他刚看到的那次修改。
   */
  /**
   * 撤销一步。
   *
   * 只说"已撤销"是不够的：过滤视图下被撤销的那条可能根本不在视野里，用户会看到
   * 列表毫无变化、以为撤销失灵（实测过）。所以这里用**前后快照对比**找出到底哪条
   * 变了，并在它不可见时明说。
   */
  async function handleUndo() {
    if (undoing) return;
    undoing = true;
    const before = new Map(
      (snapshot?.slips ?? []).map((slip) => [
        slip.id,
        `${slip.text}\u0000${slip.done}`,
      ]),
    );
    try {
      // 撤销必须走同一条串行队列：勾选/归档还在飞时直接发 undo，后端会先撤销上一条
      // 已提交的变更，用户看到的是"撤错了"。队列保证撤销排在已提交的变更之后。
      const ack = await mutating.run(() => bridge.undo(bridge.newRequestId()));
      await refresh();
      if (ack.result === "applied") {
        hint = DEFAULT_HINT;
        // 撤销成功 = 连击计数 -1（R100）：还有剩余就继续挂撤销入口，
        // 减到 0 就只留一句"已撤销"——再挂按钮就是必然失败的按钮。
        undoTrail = dropUndoBurst(undoTrail, Date.now());
        notice = undoTrail
          ? undoNotice(describeUndo(before), undoTrail.count)
          : { message: describeUndo(before), undo: false };
      } else {
        // 撤销被后端拒绝（通常是栈已经空了）：本地计数已经不可信，直接清掉，
        // 不能让提示条继续声称"还有 3 步可撤销"。
        undoTrail = null;
        hint = `撤销失败：${formatAckError(ack)}`;
      }
    } catch (error) {
      hint = `undo 失败：${error}`;
    } finally {
      undoing = false;
    }
  }

  /**
   * 从"撤销前后"的快照差里找出被动到的那一条，并据此组织一句有用的提示。
   *
   * 找不到差异（例如撤销的是归档导致的隐藏）时退回"已撤销"——不能编造是哪条。
   */
  function describeUndo(before) {
    const changed = (snapshot?.slips ?? []).filter((slip) => {
      const was = before.get(slip.id);
      const now = `${slip.text}\u0000${slip.done}`;
      return was !== undefined && was !== now;
    });
    const affected = changed[0];
    if (!affected) return "已撤销";
    const hidden = !visible.some((slip) => slip.id === affected.id);
    const name =
      affected.text.length > 12
        ? `${affected.text.slice(0, 12)}…`
        : affected.text;
    return hidden
      ? `已撤销：「${name}」（不在当前过滤结果里）`
      : `已撤销：「${name}」`;
  }

  /**
   * 窗口级快捷键：
   * - `Ctrl+Z`：非输入态时触发后端撤销；
   * - `Ctrl+Shift+D`：切换 §9.7 窗口验收调试面板。
   */
  function handleKeydown(event) {
    // 损坏预览是只读证据：弹窗开着时窗口级快捷键（Ctrl+Z / Ctrl+F / Alt+Z）一律让路，
    // 否则用户在"看证据"时会真的改掉后端内容，或在弹窗背后打开过滤框。
    if (preview) return;
    const lowered = event.key.toLowerCase();
    // Alt+Z：精准响应当前提示条上那个按钮（有撤销就撤销，纯提示就收起）。
    // 之所以还要 Alt 组合：Ctrl+Z 是"撤销"的通用语义，而提示条有时只是通知，没有可撤销的。
    if (
      event.altKey &&
      !event.ctrlKey &&
      !event.shiftKey &&
      !event.metaKey &&
      lowered === "z"
    ) {
      const action = noticeAction(notice);
      if (action === "none") return;
      event.preventDefault();
      if (action === "undo") {
        handleUndo();
      } else if (action === "restore") {
        restoreDraft();
      } else {
        notice = null;
      }
      return;
    }
    if (!event.ctrlKey || event.shiftKey || event.altKey) return;
    const key = lowered;
    if (key !== "z" && key !== "f") return;
    if (listEditing) return;
    const target = event.target;
    const tag = target?.tagName;
    if (tag === "INPUT" || tag === "TEXTAREA" || target?.isContentEditable) {
      return;
    }
    event.preventDefault();
    if (key === "f") {
      filtering = true;
      // 过滤框是条件渲染出来的，等一帧再聚焦。
      tick().then(() => filterRef?.focus());
      return;
    }
    handleUndo();
  }

  /** 过滤条里按 Esc：整体退出过滤（清词 + 收起），焦点交回捕获框。 */
  function handleFilterKeydown(event) {
    const intent = filterKeyIntent(event.key);
    if (intent === "exit") {
      event.preventDefault();
      filter = "";
      filtering = false;
      captureRef?.focus();
      return;
    }
    if (intent === "focusFirst") {
      // 词已经打好了，焦点直接交给列表首条——过滤词**保留**，用户还能接着改。
      event.preventDefault();
      listRef?.focusFirst();
    }
  }

  async function handleReload() {
    hint = "重新载入中…";
    try {
      const ack = await mutating.run(() =>
        bridge.reload(bridge.newRequestId()),
      );
      hint =
        ack.result === "applied"
          ? "已重新载入"
          : `重载失败：${formatAckError(ack)}`;
      if (ack.result === "applied") {
        // 重新载入等同于换了一份内容：撤销栈清空，连击计数一起清（既有清栈契约）。
        notice = null;
        undoTrail = null;
        // 归档抽屉、只读预览都基于旧内容，换内容后必须退出。
        showArchived = false;
        preview = null;
      }
    } catch (error) {
      hint = `重载失败：${error}`;
    }
    await refresh();
  }

  /** 「隔离并重建」的两步确认入口（与另外两处共用 `confirmGate`）。 */
  function handleIsolateClick() {
    if (confirmGate("isolate")) handleIsolate();
  }

  async function handleIsolate() {
    hint = "正在隔离并重建…";
    try {
      const ack = await mutating.run(() =>
        bridge.isolateAndReset(bridge.newRequestId()),
      );
      hint =
        ack.result === "applied"
          ? "已重建空 Inbox"
          : `重建失败：${formatAckError(ack)}`;
      if (ack.result === "applied") {
        // 隔离重建 = 内容全换，撤销栈没有意义，连击计数一并清空。
        notice = null;
        undoTrail = null;
        showArchived = false;
        preview = null;
      }
    } catch (error) {
      hint = `重建失败：${error}`;
    }
    await refresh();
  }

  /**
   * 在资源管理器里选中最新的损坏文件。
   *
   * 定位失败不改动任何界面状态，只在提示区说一句——这个按钮是"帮我找到证据"，
   * 失败了也不影响正文的读写。
   */
  async function handleRevealCorrupt() {
    try {
      await bridge.revealCorrupt();
    } catch (error) {
      hint = `定位失败：${error}`;
    }
  }

  /**
   * 打开正文里的链接。
   *
   * 成败都不改界面状态：失败只在提示区说一句——开浏览器不是内容操作，
   * 失败不该影响纸条本身。
   */
  async function handleOpenLink(url) {
    try {
      await bridge.openUrl(url);
    } catch (error) {
      hint = `打不开链接：${error}`;
    }
  }

  /**
   * 打开损坏文件的**只读**预览（R108）。
   *
   * 路径必须拼成"vault 目录 + 文件名"：后端的 `guard_corrupt_path` 会校验父目录
   * 就是 vault，且文件名以 `inbox.md.corrupt-` 开头。只给文件名会被拒。
   * 读失败也留在弹窗里报告（而不是把弹窗关掉）——用户点进来是因为想知道"还剩什么"，
   * 关掉只会让他更迷惑。
   */
  async function openCorruptPreview(name) {
    let text;
    try {
      text = await bridge.corruptPreview(bridge.vaultFilePath(vaultDir, name));
    } catch (error) {
      // 读失败也留在弹窗里报告，而不是把弹窗关掉——用户点进来就是想看"还剩什么"。
      text = `打不开：${error}`;
    }
    preview = { name, text };
    // 内容一到就聚焦面板并全选：这个弹窗唯一的用途就是"看清并拷走"，全选省掉一次
    // 手动拖选。webview 没有剪贴板权限，所以走系统选区 + Ctrl+C。
    //
    // 焦点必须留在弹窗里：面板有 `tabindex="-1"`，聚焦它之后 Tab 链以弹窗为起点，
    // 而不是继续在弹窗背后的捕获框/列表上走。
    await tick();
    previewPanelEl?.focus();
    if (previewEl) globalThis.getSelection?.()?.selectAllChildren(previewEl);
  }

  /**
   * 弹窗开着时，Escape 必须只关弹窗。
   *
   * 用**捕获阶段**拦：捕获框自己也有 Escape（放弃这次输入并收窗口），
   * 冒泡阶段拦的话它已经先跑了——弹窗关掉的同时窗口也收走，用户会以为界面出错。
   */
  function handleKeydownCapture(event) {
    // 弹窗打开时 Tab 只许在弹窗内循环：既不能让焦点跑到背后的捕获框/列表，
    // 也不能把唯一的「关闭」按钮锁在键盘链之外（旧实现一律 preventDefault，
    // 键盘用户永远到不了按钮，只能猜 Esc）。
    if (event.key === "Tab" && preview) {
      const focusables = Array.from(
        previewPanelEl?.querySelectorAll(
          'button, [href], input, select, textarea, [tabindex]:not([tabindex="-1"])',
        ) ?? [],
      ).filter((element) => !element.disabled);
      if (focusables.length === 0) {
        event.preventDefault();
        previewPanelEl?.focus();
        return;
      }
      const first = focusables[0];
      const last = focusables[focusables.length - 1];
      const active = document.activeElement;
      const outside = !previewPanelEl?.contains(active);
      if (
        event.shiftKey &&
        (active === first || active === previewPanelEl || outside)
      ) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && (active === last || outside)) {
        event.preventDefault();
        first.focus();
      }
      return;
    }
    if (event.key !== "Escape" || !preview) return;
    event.preventDefault();
    event.stopPropagation();
    preview = null;
  }

  onMount(() => {
    let disposed = false;
    let unlisten;

    // 回到前台就跑一次静默刷新：休眠/长时间切走期间，磁盘上的文件可能已被别的程序改过。
    const onFocus = () => {
      void silentRefresh();
      void refreshHotkeyStatus();
    };
    const onVisibility = () => {
      if (document.visibilityState === "visible") void silentRefresh();
    };
    window.addEventListener("focus", onFocus);
    document.addEventListener("visibilitychange", onVisibility);

    // 启动时取一次；运行期托盘换键后靠 onFocus / summon 重取。
    void refreshHotkeyStatus();

    // 事件订阅也走 bridge：组件不直接碰 `__TAURI__`。
    bridge
      .subscribeSummon(() => summon())
      .then((off) => {
        // 注册和卸载可能赛跑：晚到的 unlisten 必须立刻兑现，
        // 否则窗口每唤出一次就多留一个监听。
        if (disposed) {
          off?.();
        } else {
          unlisten = off;
        }
      })
      .catch(() => {});

    // 损坏文件只在启动时看一次。读失败就当没有：它是提示不是门禁，
    // 不该因为一次目录读取失败就在界面上报警。
    bridge
      .corruptFiles()
      .then((list) => {
        if (!disposed) corrupt = Array.isArray(list) ? list : [];
      })
      .catch(() => {});

    (async () => {
      if (!bridge.available) {
        hint = "不在 Tauri 中运行：后端命令不可用。";
        return;
      }
      try {
        const full = await bridge.vaultPath();
        vaultLabel = full.split(/[\\/]/).pop() ?? "";
        // 全路径另存一份：R108 的预览要把它和文件名拼起来给后端做目录校验。
        vaultDir = full;
      } catch {
        vaultLabel = "";
        vaultDir = "";
      }
      await refresh();
      captureRef?.focus();
    })();

    return () => {
      disposed = true;
      unlisten?.();
      window.removeEventListener("focus", onFocus);
      document.removeEventListener("visibilitychange", onVisibility);
      // 一次性定时器会在几秒内自灭，但组件先卸载时它就是悬空回调。
      for (const timer of Object.values(armTimers)) clearTimeout(timer);
    };
  });
</script>

<svelte:window
  onkeydown={handleKeydown}
  onkeydowncapture={handleKeydownCapture}
/>

<div class="paper" class:paper--leaving={leaving}>
  <header class="paper__bar" data-tauri-drag-region="deep">
    <span class="paper__title">Slip</span>
    <div class="paper__bar-right">
      <button
        type="button"
        class="paper__archive-all"
        onclick={handleCopyDiagnostics}
        title="把 vault 路径、版本号、健康与诊断信息复制到剪贴板"
      >
        复制诊断
      </button>
      <button
        type="button"
        class="paper__archive-all"
        onclick={toggleBackups}
        title="查看与恢复历史备份"
      >
        {showBackups ? "关闭备份" : "备份"}
      </button>
      {#if activeSlips.length > 0}
        <button
          type="button"
          class="paper__archive-all"
          onclick={handleExport}
          title="导出为可读 Markdown（不含内部标记）"
        >
          导出
        </button>
      {/if}
      {#if archivedSlips.length > 0 || showArchived}
        <!-- 归档抽屉：归档只是墓碑，得能翻回来看，也得能恢复。
             条件含 `showArchived`：整库替换后归档数可能变成 0，若仍按数量渲染，
             「返回列表」会消失，用户被留在空抽屉里没有出口。 -->
        <button
          type="button"
          class="paper__archive-all"
          onclick={toggleArchived}
          title={showArchived ? "回到未归档列表" : "查看已归档的纸条"}
        >
          {showArchived ? "返回列表" : `已归档 ${archivedSlips.length}`}
        </button>
      {/if}
      {#if doneCount > 0 && !blockedReason}
        <button
          type="button"
          class="paper__archive-all"
          class:confirm-danger={armed.archive !== null}
          onclick={handleArchiveAllClick}
          title={armed.archive !== null
            ? "再点一次确认：把已完成的全部收进归档（可撤销）"
            : "将所有已完成的纸条归档收起"}
        >
          {armed.archive !== null
            ? `确认归档 (${doneCount})？`
            : `归档已完成 (${doneCount})`}
        </button>
      {/if}
      {#if filtering && matchPending.length > 0 && !blockedReason}
        <!--
          R96：过滤结果里还有未完成项时才出现。两步确认（4 秒自动还原），
          这一批在后端撤销栈里是**一步**——撤销一次整批回退。
        -->
        <button
          type="button"
          class="paper__archive-all"
          class:confirm-danger={armed.complete !== null}
          onclick={handleCompleteManyClick}
          title={armed.complete !== null
            ? "再点一次确认：把匹配到的未完成项全部标为完成（可撤销）"
            : `把当前过滤结果里的 ${matchPending.length} 条未完成项一次标记完成`}
        >
          {armed.complete !== null
            ? `确认完成 (${matchPending.length})？`
            : `完成匹配项 (${matchPending.length})`}
        </button>
      {/if}
      <span class="paper__meta" title={metaTitle}>{meta}</span>
      {#if corrupt.length > 0}
        <!-- 只在真的留下过损坏文件时出现。title 里列文件名，方便核对是哪些。 -->
        <button
          type="button"
          class="paper__link"
          title={`已隔离的文件：\n${corrupt.join("\n")}`}
          onclick={handleRevealCorrupt}
        >
          定位
        </button>
        <!--
          R108：看一眼残存内容。**只读**——能看能复制，没有任何写回入口。
          默认预览最新的那个（`corrupt` 由后端按新的在前给出）。
        -->
        <button
          type="button"
          class="paper__link"
          title="只读查看最新损坏文件的残存内容（不会写回）"
          onclick={() => openCorruptPreview(corrupt[0])}
        >
          残存内容
        </button>
      {/if}
    </div>
  </header>

  <section class="capture">
    {#if banner}
      <div
        class="banner"
        class:banner--error={bannerLevel === "error"}
        class:banner--warn={bannerLevel === "warn"}
        class:banner--info={bannerLevel === "info"}
        role={bannerLevel === "error" ? "alert" : "status"}
      >
        <div class="banner__body">
          <span>{banner}</span>
          {#if diagnostics.length > 0}
            <div class="banner__diag">{diagnostics.join("；")}</div>
          {/if}
          {#if shouldShowReadonlyGuide(snapshot)}
            <!--
              排查指引用 <details> 折叠：默认收起，不抢横幅的注意力；要展开才占空间。
              纯前端，不需要任何新 capability。
            -->
            <details class="banner__guide">
              <summary>排查指引</summary>
              <ol>
                <li>
                  关掉可能占用这个文件的程序（编辑器、OneDrive
                  同步、杀毒软件的实时扫描）。
                </li>
                <li>
                  在资源管理器里右键 <code>inbox.md</code> → 属性 → 取消勾选「只读」→
                  确定。
                </li>
                <li>若文件在同步盘（OneDrive）里，先暂停同步再试。</li>
                <li>回到这里点「重新载入」。</li>
              </ol>
            </details>
          {/if}
        </div>
        {#if snapshot?.runtime?.state !== "fatal"}
          <div class="banner__actions">
            {#if snapshot?.recovery?.state === "isolated"}
              <button type="button" onclick={handleIsolate}>重试重建</button>
            {:else if snapshot && !snapshot.health.writable}
              <button type="button" onclick={handleReload}>重新载入</button>
              <button
                type="button"
                class:confirm-danger={armed.isolate !== null}
                title={armed.isolate !== null
                  ? "再点一次确认：当前内容会被清空重建"
                  : "把当前文件隔离保存，并重建一个空 Inbox"}
                onclick={handleIsolateClick}
                >{armed.isolate !== null ? "确认重建？" : "隔离并重建"}</button
              >
            {:else if snapshot?.needsReload}
              <button type="button" onclick={handleReload}>重新载入</button>
            {/if}
          </div>
        {/if}
      </div>
    {/if}
    <Capture
      bind:this={captureRef}
      hint={blockedReason || hint}
      disabled={Boolean(blockedReason)}
      onsubmit={handleCapture}
      onescape={abandon}
      onseeklist={() => listRef?.focusFirst()}
      ondraftcleared={(text) => {
        // R98：清空草稿不是"可撤销操作"（后端没有这一步），所以走一条**本地**提示：
        // 文案 + 一次性"恢复草稿"。连击计数不动——之前那些步骤在后端仍然可撤。
        // 提示条就在捕获框正下方，提示区（hint）不再重复同一句话。
        notice = { message: "草稿已清空", restore: text };
        hint = DEFAULT_HINT;
      }}
    />
    {#if notice}
      <div class="notice" class:notice--leaving={noticeLeaving}>
        <span>{notice.message}</span>
        {#if notice.undo}
          <button type="button" onclick={handleUndo} disabled={undoing}>
            撤销
          </button>
        {:else if typeof notice.restore === "string"}
          <!-- R98：清空草稿的恢复入口，单次——恢复一次之后提示条立刻消失。 -->
          <button type="button" onclick={restoreDraft}>恢复草稿</button>
        {/if}
      </div>
    {/if}
    {#if filtering}
      <!-- 纯视图过滤：只筛显示，不动快照、撤销栈与磁盘。 -->
      <div class="filter">
        <input
          bind:this={filterRef}
          bind:value={filter}
          class="filter__input"
          type="text"
          placeholder="过滤纸条…（Esc 退出）"
          aria-label="过滤纸条"
          onkeydown={handleFilterKeydown}
        />
        <span class="filter__count">{visible.length}</span>
      </div>
    {/if}
  </section>

  {#if showBackups}
    <ul class="slips">
      {#if backups.length === 0}
        <li class="slips__empty">
          <p class="slips__empty-title">还没有备份</p>
          <p class="slips__empty-foot">每次保存都会留一代，最多保留 5 代</p>
        </li>
      {:else}
        {#each backups as backup (backup.name)}
          <li class="slip">
            <time
              class="slip__time"
              datetime={new Date(backup.stampMs).toISOString()}
              >{formatStamp(backup.stampMs)}</time
            >
            <span class="slip__text">
              {backup.slips} 条 · {formatSize(backup.size)}
              {#if backup.preview}<span class="slip__hint"
                  >{backup.preview}</span
                >{/if}
              {#if pendingRestore === backup.name}
                <!--
                  R109：二次确认里把代价说清楚——覆盖的是"当前 N 条未完成项"。
                  N 由快照现算（未归档且未完成）；拿不到时由纯函数给出"无法统计"，
                  不编数字。
                -->
                <span class="slip__hint slip__hint--warn"
                  >{restoreCoverWarning(pendingCount)}</span
                >
              {/if}
            </span>
            <button
              type="button"
              class="slip__archive"
              class:confirm-danger={pendingRestore === backup.name}
              aria-label={pendingRestore === backup.name
                ? "确认恢复"
                : "恢复这个备份"}
              title={pendingRestore === backup.name
                ? `再点一次确认：${restoreCoverWarning(pendingCount)}`
                : "用这个备份覆盖当前内容"}
              onclick={() => handleRestore(backup)}
            >
              {pendingRestore === backup.name ? "确认？" : "恢复"}
            </button>
          </li>
        {/each}
      {/if}
    </ul>
  {:else}
    <SlipList
      bind:this={listRef}
      slips={visible}
      filtering={filter.trim().length > 0}
      query={filter}
      archivedView={showArchived}
      archivedCount={archivedSlips.length}
      onunarchive={handleUnarchive}
      ontoggle={handleToggle}
      onedit={handleEdit}
      onlock={handleLock}
      onpin={handlePin}
      onarchive={handleArchive}
      onpurge={handlePurgeClick}
      {purgeArmedId}
      onseekcapture={() => captureRef?.focus()}
      ondelink={handleOpenLink}
      onswipecomplete={handleSwipeComplete}
      onnotice={(message) => (hint = message)}
      bind:editing={listEditing}
    />
  {/if}

  {#if showArchived && capacityLine && !showBackups}
    <!--
      R104：抽屉底部的容量摘要。文案由 `capacitySummary` 生成；
      读不到容量时 `capacityLine` 是空串，整行不出现（不拿 0 冒充）。
    -->
    <p class="archive-summary">{capacityLine}</p>
  {/if}

  {#if preview}
    <!--
      R108：损坏文件的**只读**残存预览。

      刻意没有输入框、没有保存按钮：这是证据，不是可编辑内容。能做的只有看和复制
      （正文可选中，打开时已全选，Ctrl+C 即可拷走）。
      "不会写回"必须写在标题上——否则用户会以为改这里能救回数据。
    -->
    <div
      class="preview"
      role="dialog"
      aria-modal="true"
      aria-label="损坏文件的残存内容（只读）"
    >
      <div class="preview__panel" tabindex="-1" bind:this={previewPanelEl}>
        <div class="preview__bar">
          <span class="preview__title"
            >{preview.name} · 残存内容（只读）· 不会写回 inbox.md · 已全选</span
          >
          <button
            type="button"
            class="paper__archive-all"
            onclick={() => (preview = null)}>关闭</button
          >
        </div>
        <pre class="preview__body" bind:this={previewEl}>{preview.text}</pre>
      </div>
    </div>
  {/if}
</div>
