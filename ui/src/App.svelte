<script>
  import { onMount } from "svelte";
  import * as bridge from "./lib/bridge.js";
  import Capture from "./lib/Capture.svelte";
  import SlipList from "./lib/SlipList.svelte";
  import SpikePanel from "./lib/SpikePanel.svelte";

  /** 默认提示文案，唤出和放弃捕获时都要回到它，所以只写一次。 */
  const DEFAULT_HINT = "Enter 收录 · Shift+Enter 换行";

  /** 后端发布的最新快照；`null` 表示还没取到。 */
  let snapshot = $state(null);
  let hint = $state(DEFAULT_HINT);
  let vaultLabel = $state("");
  let captureRef = $state(null);
  /** 正在等待应答的提交；迟到 ACK 必须与这条不可变记录比较。 */
  let inFlight = $state(null);

  let visible = $derived((snapshot?.slips ?? []).filter((slip) => !slip.archived));
  let pendingCount = $derived(visible.filter((slip) => !slip.done).length);

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
      return "文件无法解析，暂时只读。可以隔离它后重建一个空 Inbox。";
    }
    if (snapshot.needsReload) {
      return "磁盘上的文件已被外部修改，需要重新载入。";
    }
    return "";
  });

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
    captureRef?.focus();
  }

  /** Escape：收走等于放弃这次捕获，残稿不能留到下次唤出。 */
  function abandon() {
    captureRef?.clear();
    hint = DEFAULT_HINT;
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
        hint = `未收录：${ack.error?.message ?? ack.result}`;
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
    try {
      await bridge.complete(bridge.newRequestId(), slip.id, !slip.done);
    } catch (error) {
      hint = `complete 失败：${error}`;
    }
    await refresh();
  }

  async function handleReload() {
    hint = "重新载入中…";
    try {
      const ack = await bridge.reload(bridge.newRequestId());
      hint = ack.result === "applied" ? "已重新载入" : `重载失败：${ack.error?.message ?? ack.result}`;
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
          : `重建失败：${ack.error?.message ?? ack.result}`;
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

<div class="paper">
  <header class="paper__bar" data-tauri-drag-region>
    <span class="paper__title">Slip</span>
    <span class="paper__meta">{meta}</span>
  </header>

  <section class="capture">
    {#if banner}
      <div class="banner">
        <span>{banner}</span>
        {#if snapshot?.runtime?.state !== "fatal"}
          {#if snapshot?.recovery?.state === "isolated" || (snapshot && !snapshot.health.writable)}
            <button type="button" onclick={handleIsolate}>隔离并重建</button>
          {:else if snapshot?.needsReload}
            <button type="button" onclick={handleReload}>重新载入</button>
          {/if}
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
  </section>

  <SlipList slips={visible} ontoggle={handleToggle} />

  <SpikePanel />
</div>
