<script>
  import { onMount } from "svelte";
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
  /** 正在等待应答的提交；迟到 ACK 必须与这条不可变记录比较。 */
  let inFlight = $state(null);
  /** 列表里的编辑态；Ctrl+Z 只在没在编辑时才抢撤销。 */
  let listEditing = $state(false);
  /** 撤销提示：与普通 hint 分开，因为只有它需要挂一个按钮。 */
  let notice = $state(null);
  /** 撤销请求在途，避免连按 Ctrl+Z 把后端的撤销栈一次抽干。 */
  let undoing = $state(false);


  // 归档判定只看 `archived`：后端 `SlipView.archived` 是唯一判据
  // （`done && archived_at.is_some()`，见 v11 §1.8），没有独立的 archivedAt 字段。
  let visible = $derived((snapshot?.slips ?? []).filter((slip) => !slip.archived));
  let pendingCount = $derived(visible.filter((slip) => !slip.done).length);
  let doneSlips = $derived(visible.filter((slip) => slip.done));
  let doneCount = $derived(doneSlips.length);

  let meta = $derived(
    [
      vaultLabel,
      `${pendingCount} 条未完成`,
      snapshot ? `rev ${snapshot.contentRev}` : "",
    ]
      .filter(Boolean)
      .join(" · "),
  );

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
      return "文件无法解析，暂时只读。修好文件后可重新载入，或隔离并重建空 Inbox。";
    }
    if (snapshot.needsReload) {
      return "磁盘上的文件已被外部修改，需要重新载入。";
    }
    return "";
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
      case "busy":
        return "操作过于频繁，请稍后再试";
      case "notExecuted":
        return "后台未执行该操作，请重试";
      case "fatal":
        return "运行时状态异常，请重启 Slip";
      case "undo":
        return msg;
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
    if (snapshot && !snapshot.health.writable) return "文件无法解析，暂时只读";
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
    const targetDone = !slip.done;
    try {
      const ack = await bridge.complete(
        bridge.newRequestId(),
        slip.id,
        targetDone,
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

  /** 一键归档当前所有已完成的纸条（同一事务，一次 Ctrl+Z 即可整批恢复）。 */
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
  async function handleUndo() {
    if (undoing) return;
    undoing = true;
    try {
      const ack = await bridge.undo(bridge.newRequestId());
      await refresh();
      if (ack.result === "applied") {
        hint = DEFAULT_HINT;
        notice = { message: "已撤销", undo: false };
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
   * 窗口级快捷键：
   * - `Ctrl+Z`：非输入态时触发后端撤销；
   * - `Ctrl+Shift+D`：切换 §9.7 窗口验收调试面板。
   */
  function handleKeydown(event) {
    if (event.key !== "z" && event.key !== "Z") return;
    if (!event.ctrlKey || event.shiftKey || event.altKey) return;
    if (listEditing) return;
    const target = event.target;
    const tag = target?.tagName;
    if (tag === "INPUT" || tag === "TEXTAREA" || target?.isContentEditable) {
      return;
    }
    event.preventDefault();
    handleUndo();
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
      <span class="paper__meta">{meta}</span>
    </div>
  </header>

  <section class="capture">
    {#if banner}
      <div class="banner">
        <div class="banner__body">
          <span>{banner}</span>
          {#if diagnostics.length > 0}
            <div class="banner__diag">{diagnostics.join("；")}</div>
          {/if}
        </div>
        {#if snapshot?.runtime?.state !== "fatal"}
          <div class="banner__actions">
            {#if snapshot?.recovery?.state === "isolated"}
              <button type="button" onclick={handleIsolate}>重试重建</button>
            {:else if snapshot && !snapshot.health.writable}
              <button type="button" onclick={handleReload}>重新载入</button>
              <button type="button" onclick={handleIsolate}>隔离并重建</button>
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
    />
    {#if notice}
      <div class="notice">
        <span>{notice.message}</span>
        {#if notice.undo}
          <button type="button" onclick={handleUndo} disabled={undoing}>
            撤销
          </button>
        {/if}
      </div>
    {/if}
  </section>

  <SlipList
    slips={visible}
    ontoggle={handleToggle}
    onedit={handleEdit}
    onlock={handleLock}
    onarchive={handleArchive}
    onnotice={(message) => (hint = message)}
    bind:editing={listEditing}
  />
</div>
