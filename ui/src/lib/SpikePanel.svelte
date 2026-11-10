<script>
  /**
   * §9.7 窗口验收面板。
   *
   * 这不是产品功能。窗口 spike 的 7 条结论一旦确定，本文件应当整体删除。
   * 现在留着是因为没有 devtools 时它是唯一能取证的地方。
   */
  import * as bridge from "./bridge.js";

  let collapsed = $state(true);
  let output = $state("");

  const show = (value) =>
    (output =
      typeof value === "string" ? value : JSON.stringify(value, null, 2));

  async function invoke(command, args) {
    try {
      return await bridge.spike(command, args);
    } catch (error) {
      show(`${command} 失败：${error}`);
      return null;
    }
  }

  async function report() {
    const value = await invoke("spike_window_report");
    if (value) {
      show(value);
    }
  }

  const actions = {
    async "ignore-on"() {
      await invoke("spike_ignore_cursor", { ignore: true });
      show(
        "点击穿透已打开：整个窗口不再接收鼠标事件。\n" +
          "要恢复，请在 CDP 控制台执行：\n" +
          "window.__TAURI__.core.invoke('spike_ignore_cursor', { ignore: false })",
      );
    },
    async "ignore-off"() {
      await invoke("spike_ignore_cursor", { ignore: false });
      show("点击穿透已关闭。");
    },
    async "top-on"() {
      await invoke("spike_always_on_top", { on: true });
      await report();
    },
    async "top-off"() {
      await invoke("spike_always_on_top", { on: false });
      await report();
    },
    report,
    async monitors() {
      show(await invoke("spike_monitors"));
    },
    async hide() {
      show("正在隐藏，1.2 秒后恢复——请盯住原应用是否拿回焦点。");
      await invoke("spike_hide");
      setTimeout(async () => {
        await invoke("spike_show");
        await report();
      }, 1200);
    },
    async show() {
      await invoke("spike_show");
      await report();
    },
    async reload() {
      await bridge.reload(bridge.newRequestId());
      show("已重新载入。");
    },
    async isolate() {
      await bridge.isolateAndReset(bridge.newRequestId());
      show("已隔离并重建空 Inbox。");
    },
  };

  async function handle(event) {
    const action = event.target?.dataset?.act;
    await actions[action]?.();
  }
</script>

<section class="spike">
  <div class="spike__head">
    <strong>§9.7 窗口验收</strong>
    <button
      type="button"
      class="spike__toggle"
      onclick={() => (collapsed = !collapsed)}
    >
      {collapsed ? "展开" : "收起"}
    </button>
  </div>

  {#if !collapsed}
    <div class="spike__row">
      <button type="button" data-act="ignore-on" onclick={handle}>
        点击穿透 开
      </button>
      <button type="button" data-act="ignore-off" onclick={handle}>
        点击穿透 关
      </button>
      <button type="button" data-act="top-on" onclick={handle}>置顶 开</button>
      <button type="button" data-act="top-off" onclick={handle}>置顶 关</button>
    </div>
    <div class="spike__row">
      <button type="button" data-act="report" onclick={handle}>读窗口状态</button>
      <button type="button" data-act="monitors" onclick={handle}>列显示器</button>
      <button type="button" data-act="hide" onclick={handle}>隐藏</button>
      <button type="button" data-act="show" onclick={handle}>显示</button>
    </div>
    <div class="spike__row">
      <button type="button" data-act="reload" onclick={handle}>重新载入文件</button>
      <button type="button" data-act="isolate" onclick={handle}>隔离并重建</button>
    </div>
    <pre class="spike__out">{output}</pre>
  {/if}
</section>
