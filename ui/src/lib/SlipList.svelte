<script>
  import { tick } from "svelte";

  /**
   * 纸片列表。
   *
   * 只负责展示与转发意图：任何"点完之后应该显示什么"都由后端快照决定，
   * 这里不做乐观更新，否则就要回答"发射丢失时以谁为准"。
   *
   * 唯一的本地状态是"正在编辑哪一条"，因为它是纯粹的界面瞬时状态：
   * 提交成功后仍然靠刷新快照拿新正文，编辑框只是把用户输入暂存起来。
   */
  let {
    slips = [],
    ontoggle,
    /** 提交正文编辑，返回是否被后端接受；返回 false 时编辑框必须留着。 */
    onedit,
    /** 切换锁定。 */
    onlock,
    /** 归档单条已完成纸条。 */
    onarchive,
    /** 往提示区送一条文案（例如锁定纸条被双击时）。 */
    onnotice,
    /** 在最上面一条按 ↑ 时把焦点交回捕获框。 */
    onseekcapture,
    /** 双向暴露"是否正在编辑"，让 App 的 Ctrl+Z 知道该不该让路。 */
    editing = $bindable(false),
  } = $props();

  let editingId = $state(null);
  /** 编辑框里的初值/当前值；提交失败时靠它把用户输入原样留着。 */
  let draft = $state("");
  /** 输入法组合态：选词按 Enter 或 Esc 时绝不能误提交或丢弃编辑。 */
  let composing = $state(false);
  /** 提交进行中，防止失焦与 Enter 各发一次请求。 */
  let committing = $state(false);
  /** Esc 取消后会立刻失焦，用这个标记让失焦不再触发提交。 */
  let cancelled = $state(false);

  /**
   * 每条正文元素，用于方向键导航。
   *
   * 用 DOM 焦点当"当前选中项"，不另建一份 `selectedId` 状态——两份状态就要回答
   * "焦点和选中不一致时听谁的"，而这问题没有正确答案。
   */
  let itemEls = $state([]);

  /** 把焦点移到第一条。捕获框在空内容按 ↓ 时由 App 调用。 */
  export function focusFirst() {
    const first = itemEls.find(Boolean);
    first?.focus();
  }

  $effect(() => {
    editing = editingId !== null;
  });

  /**
   * 自动聚焦并全选。
   *
   * 用 action 而不是 `$effect` + `tick`：编辑框是条件渲染出来的，
   * action 恰好在它挂载那一刻执行，不需要额外等待一轮刷新。
   */
  function focusAndSelect(node) {
    node.focus();
    node.select();
  }

  function beginEdit(slip) {
    // 锁定只保护正文；这里要明确告诉用户为什么双击没反应，否则像是坏了。
    if (slip.locked) {
      onnotice?.("已锁定，先解锁才能修改");
      return;
    }
    editingId = slip.id;
    draft = slip.text;
    composing = false;
    cancelled = false;
  }

  /** 取消不产生任何请求：编辑框里的内容不是用户想保存的东西。 */
  function cancelEdit() {
    cancelled = true;
    composing = false;
    editingId = null;
    draft = "";
  }

  async function commitEdit() {
    // 失焦可能和 Enter 撞在一起；也承接"取消后立刻失焦"的情况。
    if (cancelled || committing || editingId === null) {
      return;
    }
    const slip = slips.find((item) => item.id === editingId);
    if (!slip) {
      editingId = null;
      return;
    }
    // 没改就不用发请求，免得平白把提示区刷成"已修改"。
    if (draft === slip.text) {
      editingId = null;
      return;
    }
    committing = true;
    const accepted = await onedit?.(slip, draft);
    committing = false;
    // 失败时保持 editingId 不变、draft 不清空：用户的输入不能丢。
    if (accepted) {
      editingId = null;
      draft = "";
    }
  }

  function handleKeydown(event) {
    // 与捕获框同口径：输入法选词期间的 Enter / Esc 归输入法自己消费。
    if (composing || event.isComposing || event.keyCode === 229) {
      return;
    }
    if (event.key === "Enter") {
      // 单行输入框里 Enter 没有别的含义；阻止冒泡避免触发窗口级快捷键。
      event.preventDefault();
      if (draft.trim().length === 0) {
        onnotice?.("纸条内容不能为空");
        return;
      }
      const id = editingId;
      // 键盘进入编辑、键盘提交，那么焦点也该由键盘交还——失焦提交不走这里，
      // 免得把用户刚点去的输入框抢回来。
      commitEdit().then(() => refocusRow(id));
    } else if (event.key === "Escape") {
      event.preventDefault();
      const id = editingId;
      cancelEdit();
      refocusRow(id);
    }
  }

  function handleBlur() {
    if (cancelled) {
      cancelled = false;
      return;
    }
    // 把内容删空后直接点外部失焦：视为放弃修改并恢复原文，避免卡在失焦的空编辑框里。
    if (draft.trim().length === 0) {
      cancelEdit();
      return;
    }
    commitEdit();
  }

  /** 编辑结束后把焦点放回那一行，键盘流才不至于断在半空。 */
  async function refocusRow(id) {
    await tick();
    const index = slips.findIndex((item) => item.id === id);
    if (index >= 0) itemEls[index]?.focus();
  }

  /**
   * 纸条正文上的方向键导航。
   *
   * Space 勾选、Enter 编辑、↑↓ 上下移动；在最上面按 ↑ 回到捕获框。
   * 这样"录入 → 下移 → 勾选 → 编辑 → 退出"全程可以不碰鼠标。
   *
   * 刻意不维护"当前选中项"状态：DOM 焦点就是它。两份状态就要回答"焦点和选中
   * 不一致时听谁的"，而那没有正确答案。
   */
  function handleItemKeydown(event, slip, index) {
    if (composing || event.isComposing || event.keyCode === 229) {
      return;
    }
    switch (event.key) {
      case "ArrowDown":
        event.preventDefault();
        itemEls[index + 1]?.focus();
        return;
      case "ArrowUp":
        event.preventDefault();
        if (index === 0) {
          onseekcapture?.();
        } else {
          itemEls[index - 1]?.focus();
        }
        return;
      case " ":
        // 空格在 span 上默认是滚动，必须挡掉，否则勾选的同时页面会跳。
        event.preventDefault();
        ontoggle?.(slip);
        return;
      case "Enter":
        event.preventDefault();
        beginEdit(slip);
        return;
      default:
        return;
    }
  }
</script>

<ul class="slips">
  {#if slips.length === 0}
    <!--
      空态不是装饰：第一次启动这张纸是空的时候，用户最容易以为"什么也没发生"。
      这里不展示具体热键——视图里的 hotkey 是**配置原值**，可能写过但没注册成功，
      显示它等于给一个可能不生效的键。真正生效的那个由托盘菜单负责展示。
    -->
    <li class="slips__empty">
      <p class="slips__empty-title">还没有内容</p>
      <ol class="slips__empty-steps">
        <li>
          按唤出热键（默认 <kbd>Ctrl</kbd>+<kbd>Alt</kbd>+<kbd>Space</kbd
          >，也可以点托盘图标）
        </li>
        <li>
          打完字按 <kbd>Enter</kbd> —— 条目立刻写进
          <code>inbox.md</code>，窗口自动收起
        </li>
      </ol>
      <p class="slips__empty-foot">
        换过热键？右键托盘图标，菜单第一行就是当前生效的那个。
      </p>
    </li>
  {:else}
    {#each slips as slip, index (slip.id)}
      <li class:slip--done={slip.done} class="slip">
        <button
          type="button"
          class="slip__box"
          class:slip__box--done={slip.done}
          aria-checked={slip.done}
          role="checkbox"
          aria-label={slip.done ? "标记为未完成" : "标记为完成"}
          onclick={() => ontoggle?.(slip)}
        ></button>

        {#if editingId === slip.id}
          <input
            class="slip__edit"
            type="text"
            bind:value={draft}
            use:focusAndSelect
            onkeydown={handleKeydown}
            onblur={handleBlur}
            oncompositionstart={() => (composing = true)}
            oncompositionend={() => (composing = false)}
            aria-label="编辑纸条"
          />
        {:else}
          <span
            class="slip__text"
            class:slip__text--locked={slip.locked}
            role="textbox"
            tabindex="0"
            aria-readonly="true"
            aria-label={slip.locked
              ? "已锁定的纸条正文"
              : "纸条正文，双击或回车编辑"}
            bind:this={itemEls[index]}
            ondblclick={() => beginEdit(slip)}
            onkeydown={(event) => handleItemKeydown(event, slip, index)}
            title={slip.locked ? "已锁定，先解锁才能修改" : "双击编辑"}
            >{slip.text}</span
          >
          {#if slip.done}
            <button
              type="button"
              class="slip__archive"
              aria-label="归档纸条"
              title="归档（从列表收起）"
              onclick={() => onarchive?.(slip)}
            >
              <svg
                viewBox="0 0 16 16"
                width="13"
                height="13"
                aria-hidden="true"
              >
                <path
                  d="M2.5 3.5h11v2.5h-11zM3.5 6v6.5a1 1 0 0 0 1 1h7a1 1 0 0 0 1-1V6M6.5 8.5h3"
                  fill="none"
                  stroke="currentColor"
                  stroke-width="1.35"
                  stroke-linecap="round"
                  stroke-linejoin="round"
                />
              </svg>
            </button>
          {/if}
          <button
            type="button"
            class="slip__lock"
            class:slip__lock--on={slip.locked}
            aria-pressed={slip.locked}
            aria-label={slip.locked ? "解锁纸条" : "锁定纸条"}
            title={slip.locked ? "解锁正文" : "锁定正文"}
            onclick={() => onlock?.(slip)}
          >
            <svg viewBox="0 0 16 16" width="13" height="13" aria-hidden="true">
              <path
                d="M4.75 7V5.25a3.25 3.25 0 0 1 6.5 0V7"
                fill="none"
                stroke="currentColor"
                stroke-width="1.4"
                stroke-linecap="round"
              />
              <rect
                x="3.5"
                y="7"
                width="9"
                height="6.5"
                rx="1.6"
                fill="none"
                stroke="currentColor"
                stroke-width="1.4"
              />
            </svg>
          </button>
        {/if}
      </li>
    {/each}
  {/if}
</ul>
