<script>
  import { onMount, tick } from "svelte";
  import {
    bannerLevel as statusBannerLevel,
    shouldShowReadonlyGuide,
  } from "./lib/status.js";
  import { noticeAction, filterKeyIntent } from "./lib/keys.js";
  import { createSerialQueue, nextToggleTarget } from "./lib/queue.js";
  import { confirmStep, CONFIRM_WINDOW_MS } from "./lib/confirm.js";
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

  // 归档判定只看 `archived`：后端 `SlipView.archived` 是唯一判据
  // （`done && archived_at.is_some()`，见 v11 §1.8），没有独立的 archivedAt 字段。
  let activeSlips = $derived(
    (snapshot?.slips ?? []).filter((slip) => !slip.archived),
  );
  let archivedSlips = $derived(
    (snapshot?.slips ?? []).filter((slip) => slip.archived),
  );
  /** 是否在看归档抽屉。归档是墓碑不是删除，所以这一层只是换个筛选方向。 */
  let showArchived = $state(false);
  /** 备份面板是否展开。 */
  let showBackups = $state(false);
  /** 备份列表；只在打开面板时取一次。 */
  let backups = $state([]);
  /** 等待二次确认的备份名——恢复会覆盖当前内容，不该一次点击就执行。 */
  let pendingRestore = $state(null);
  /**
   * 「隔离并重建」的二次确认时间戳。`null` = 未武装。
   *
   * 为什么必须拦一下：这一下会把当前 inbox 换成空文件，手滑点错就是"当前内容没了"。
   */
  let isolateArmedAt = $state(null);
  let isolateArmTimer = null;

  /**
   * 过滤关键字。**纯视图**：只影响显示，不影响快照、撤销栈与磁盘。
   *
   * 撤销按后端真实提交顺序记录；过滤若参与排序或索引，`Ctrl+Z` 就会撤错东西。
   */
  let filter = $state("");
  /** 过滤条是否展开。Ctrl+F 展开，Esc 收起。 */
  let filtering = $state(false);
  let filterRef = $state(null);

  let visible = $derived.by(() => {
    const base = showArchived ? archivedSlips : activeSlips;
    const needle = filter.trim().toLowerCase();
    if (!needle) return base;
    return base.filter((slip) => slip.text.toLowerCase().includes(needle));
  });
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
      const at = Date.parse(slip.createdAt);
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

  /** 把差异条数说成人话；三项都为 0 时返回空串（那就没什么好说的）。 */
  function describeDiff(diff) {
    return [
      diff.added > 0 ? `新增 ${diff.added}` : "",
      diff.removed > 0 ? `消失 ${diff.removed}` : "",
      diff.changed > 0 ? `改动 ${diff.changed}` : "",
    ]
      .filter(Boolean)
      .join("、");
  }

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
    return "";
  });

  // 判据抽在 lib/status.js：纯函数，能脱离应用验证（tools/check-ui-logic.mjs）。
  let bannerLevel = $derived(statusBannerLevel(snapshot));

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
      case "busy":
        return "操作过于频繁，请稍后再试";
      case "notExecuted":
        return "后台未执行该操作，请重试";
      case "fatal":
        return "运行时状态异常，请重启 Slip";
      case "undo":
        return msg;
      case "blocking":
        // 后端的 message 已经是人类可读的句子（BlockingError 实现了 Display），
        // 这里只补一句"该怎么办"。
        return `inbox.md 有无法处理的结构问题：${msg}。修好文件后重新载入`;
      case "io":
        return `磁盘读写失败（${msg.replace(/^i\/o error:\s*/, "")}）`;
      case "notDegraded":
        return "当前文件状态正常，不需要隔离";
      case "domain":
        if (msg.includes("locked")) return "纸条已锁定，请先解锁再修改";
        if (msg.includes("empty")) return "纸条内容不能为空";
        if (msg.includes("not done")) return "只有已完成的纸条才能归档";
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
  function dismissWindow() {
    bridge.dismissWindow().catch(() => {});
  }

  /**
   * 热键唤出：保留上次没提交的草稿，只把光标放回去。
   *
   * 失焦自动收走是"暂时离开"，不是"放弃输入"；只有 Esc 才清空草稿。
   */
  function summon() {
    hint = DEFAULT_HINT;
    // 撤销提示跨过"收起再唤出"就过期了：那次的修改上下文已经不在了。
    notice = null;
    // 视图状态也不该跨过这一次唤出：过滤词、归档抽屉、备份面板都是**当次浏览**的临时
    // 状态。留着它们，下一次唤出会直接落在一张被筛过的列表上——看上去像"东西没了"。
    // 实测过：开着过滤收起窗口，再唤出只看到 1 条，而实际有 2 条。
    filter = "";
    filtering = false;
    showArchived = false;
    showBackups = false;
    pendingRestore = null;
    captureRef?.focus();
  }

  /** Escape：收走等于放弃这次捕获，残稿不能留到下次唤出。 */
  function abandon() {
    captureRef?.clear();
    hint = DEFAULT_HINT;
    notice = null;
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
        hint = `未收录：${formatAckError(ack)}`;
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
      hint = `capture 失败：${error}`;
    } finally {
      inFlight = null;
    }

    if (clearDraft) {
      dismissWindow();
    } else {
      captureRef?.focus();
    }
  }

  async function handleToggle(slip) {
    // 以"未落盘的意图"为准，而不是当前快照：连点两下时快照还没跟上。
    const targetDone = nextToggleTarget(slip.done, pendingDone[slip.id]);
    pendingDone[slip.id] = targetDone;
    try {
      // 串行发送：后一次点击一定排在前一次之后，不会两个请求同时基于同一份旧快照。
      const ack = await mutating.run(() =>
        bridge.complete(bridge.newRequestId(), slip.id, targetDone),
      );
      await refresh();
      if (ack.result === "applied") {
        hint = DEFAULT_HINT;
        notice = {
          message: targetDone ? "已完成" : "已标为未完成",
          undo: true,
        };
      } else {
        hint = `未更新：${formatAckError(ack)}`;
      }
    } catch (error) {
      hint = `complete 失败：${error}`;
      await refresh();
    } finally {
      // 快照已经等于意图就清掉记录；还不等（说明后面还有排队中的点击）就留着，
      // 下一次反转仍然以它为准。
      const stored = snapshot?.slips.find((item) => item.id === slip.id);
      if (stored && stored.done === targetDone) {
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
      const ack = await bridge.edit(bridge.newRequestId(), slip.id, text);
      await refresh();
      if (ack.result === "applied") {
        hint = DEFAULT_HINT;
        notice = { message: "已修改", undo: true };
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
      const ack = await bridge.setLocked(
        bridge.newRequestId(),
        slip.id,
        !slip.locked,
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
      const ack = await bridge.archive(bridge.newRequestId(), [slip.id]);
      await refresh();
      if (ack.result === "applied") {
        hint = DEFAULT_HINT;
        notice = { message: "已归档", undo: true };
      } else {
        hint = `未归档：${formatAckError(ack)}`;
      }
    } catch (error) {
      hint = `archive 失败：${error}`;
      await refresh();
    }
  }

  /** 打开/收起备份面板；打开时现取一遍列表。 */
  async function toggleBackups() {
    showBackups = !showBackups;
    pendingRestore = null;
    if (showBackups) {
      try {
        backups = await bridge.backupList();
      } catch (error) {
        backups = [];
        hint = `读取备份失败：${error}`;
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
      const ack = await bridge.restoreBackup(
        bridge.newRequestId(),
        backup.name,
      );
      await refresh();
      if (ack.result === "applied") {
        showBackups = false;
        hint = "已从备份恢复（恢复前的版本也已留档）";
        notice = null;
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
      const ack = await bridge.unarchive(bridge.newRequestId(), slip.id);
      await refresh();
      if (ack.result === "applied") {
        hint = DEFAULT_HINT;
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

  async function handleArchiveAll() {
    const ids = doneSlips.map((slip) => slip.id);
    if (ids.length === 0) return;
    try {
      const ack = await bridge.archive(bridge.newRequestId(), ids);
      await refresh();
      if (ack.result === "applied") {
        hint = DEFAULT_HINT;
        notice = { message: `已归档 ${ids.length} 条`, undo: true };
      } else {
        hint = `未归档：${formatAckError(ack)}`;
      }
    } catch (error) {
      hint = `archive 失败：${error}`;
      await refresh();
    }
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
      const ack = await bridge.undo(bridge.newRequestId());
      await refresh();
      if (ack.result === "applied") {
        hint = DEFAULT_HINT;
        notice = { message: describeUndo(before), undo: false };
      } else {
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
      const ack = await bridge.reload(bridge.newRequestId());
      hint =
        ack.result === "applied"
          ? "已重新载入"
          : `重载失败：${formatAckError(ack)}`;
      if (ack.result === "applied") {
        notice = null;
      }
    } catch (error) {
      hint = `重载失败：${error}`;
    }
    await refresh();
  }

  /** 二次确认的入口：第一下武装，第二下（4 秒内）才真的执行。 */
  function handleIsolateClick() {
    const step = confirmStep({ armedAt: isolateArmedAt, now: Date.now() });
    clearTimeout(isolateArmTimer);
    if (step === "execute") {
      isolateArmedAt = null;
      handleIsolate();
      return;
    }
    isolateArmedAt = Date.now();
    isolateArmTimer = setTimeout(() => {
      isolateArmedAt = null;
    }, CONFIRM_WINDOW_MS);
  }

  async function handleIsolate() {
    hint = "正在隔离并重建…";
    try {
      const ack = await bridge.isolateAndReset(bridge.newRequestId());
      hint =
        ack.result === "applied"
          ? "已重建空 Inbox"
          : `重建失败：${formatAckError(ack)}`;
      if (ack.result === "applied") {
        notice = null;
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

  onMount(() => {
    let disposed = false;
    let unlisten;

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
        vaultLabel = (await bridge.vaultPath()).split(/[\\/]/).pop() ?? "";
      } catch {
        vaultLabel = "";
      }
      await refresh();
      captureRef?.focus();
    })();

    return () => {
      disposed = true;
      unlisten?.();
    };
  });
</script>

<svelte:window onkeydown={handleKeydown} />

<div class="paper">
  <header class="paper__bar" data-tauri-drag-region>
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
      {#if archivedSlips.length > 0}
        <!-- 归档抽屉：归档只是墓碑，得能翻回来看，也得能恢复。 -->
        <button
          type="button"
          class="paper__archive-all"
          onclick={() => (showArchived = !showArchived)}
          title={showArchived ? "回到未归档列表" : "查看已归档的纸条"}
        >
          {showArchived ? "返回列表" : `已归档 ${archivedSlips.length}`}
        </button>
      {/if}
      {#if doneCount > 0 && !blockedReason}
        <button
          type="button"
          class="paper__archive-all"
          onclick={handleArchiveAll}
          title="将所有已完成的纸条归档收起"
        >
          归档已完成 ({doneCount})
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
                class:confirm-danger={isolateArmedAt !== null}
                title={isolateArmedAt !== null
                  ? "再点一次确认：当前内容会被清空重建"
                  : "把当前文件隔离保存，并重建一个空 Inbox"}
                onclick={handleIsolateClick}
                >{isolateArmedAt !== null ? "确认重建？" : "隔离并重建"}</button
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
    />
    {#if notice}
      <div class="notice" class:notice--leaving={noticeLeaving}>
        <span>{notice.message}</span>
        {#if notice.undo}
          <button type="button" onclick={handleUndo} disabled={undoing}>
            撤销
          </button>
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
            </span>
            <button
              type="button"
              class="slip__archive"
              class:confirm-danger={pendingRestore === backup.name}
              aria-label={pendingRestore === backup.name
                ? "确认恢复"
                : "恢复这个备份"}
              title={pendingRestore === backup.name
                ? "再点一次确认覆盖当前内容"
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
      archivedView={showArchived}
      archivedCount={archivedSlips.length}
      onunarchive={handleUnarchive}
      ontoggle={handleToggle}
      onedit={handleEdit}
      onlock={handleLock}
      onarchive={handleArchive}
      onseekcapture={() => captureRef?.focus()}
      ondelink={handleOpenLink}
      onnotice={(message) => (hint = message)}
      bind:editing={listEditing}
    />
  {/if}
</div>
