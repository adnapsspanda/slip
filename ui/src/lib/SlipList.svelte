<script>
  import { listNavTarget, tabTarget } from "./keys.js";
  import { tick } from "svelte";
  import { exactLocal, relativeTime } from "./time.js";
  import { buildInline } from "./render.js";
  import { parseQuery } from "./filter.js";
  import { extractTags } from "./tags.js";
  import { swipeIntent, swipeOffset } from "./swipe.js";

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
    /**
     * 切换置顶（R95）。置顶**不进撤销栈**，所以上层不会为它提供"撤销"——
     * 这里只负责把意图交出去，不对结果做任何乐观假设。
     */
    onpin,
    /** 归档单条已完成纸条。 */
    onarchive,
    /** 往提示区送一条文案（例如锁定纸条被双击时）。 */
    onnotice,
    /** 在最上面一条按 ↑ 时把焦点交回捕获框。 */
    onseekcapture,
    /** 点正文里的链接：交给上层用系统浏览器打开。 */
    ondelink,
    /**
     * 触控右滑过阈值（R114）。只有触摸/笔会走到这里——鼠标没有 touch 事件，
     * 所以拖拽与双击编辑完全不受影响。
     */
    onswipecomplete,
    /** 列表为空是因为过滤没匹配到，而不是真的没有内容。 */
    filtering = false,
    /**
     * 过滤框的原始输入。只用于渲染层的两件事：高亮命中片段、以及"这一行和非空
     * 模板无关"的标签徽章不受影响。**判定不在这里做**——`App.svelte` 已经用同一个
     * 纯函数筛过一遍，列表拿到的 `slips` 就是结果。
     */
    query = "",
    /** 正在看归档抽屉：每行给出"恢复"而不是"归档"。 */
    archivedView = false,
    /** 归档里有多少条。列表空但归档非空时，空态文案必须说清"东西还在"。 */
    archivedCount = 0,
    /** 把一条已归档的纸条恢复回列表。 */
    onunarchive,
    /**
     * 彻底删除一条归档项（R104）。**不可逆**，二次确认由上层闸门负责；
     * 这里只负责把意图交出去，并让"已武装"的那一行看得见。
     */
    onpurge,
    /** 已被武装待彻底删除的那一条 id；`null` 表示没有。 */
    purgeArmedId = null,
    /** 双向暴露"是否正在编辑"，让 App 的 Ctrl+Z 知道该不该让路。 */
    editing = $bindable(false),
  } = $props();

  /**
   * 过滤条件**只解析一次**，供每一行的高亮切片复用。
   *
   * 放在组件顶层而不是在模板里对每一行调 `parseQuery`：过滤框每敲一个字符都会重渲染
   * 整个列表，逐行解析等于把同一份输入重复解析 N 遍。
   */
  let parsedFilter = $derived(parseQuery(query));

  /**
   * 触控滑动状态：`{ id, x, y, decided, dx }`，`null` = 没有手势进行中。
   *
   * 为什么要 `decided`：手指刚落下时方向还没定。必须等位移超过一个小的容差、
   * 并确认横向分量更大，才能把这个手势判给"右滑完成"——否则手指稍微抖一下，
   * 一次想滚动列表的滑动就会被当成"完成"。
   */
  let swipe = $state(null);

  /** 方向判定容差（像素）。小于它时既不算横也不算竖。 */
  const SWIPE_SLOP = 12;

  function touchStart(event, slip) {
    if (event.touches.length !== 1) {
      // 多指（缩放/双指滚动）不该被理解成滑动。
      swipe = null;
      return;
    }
    const touch = event.touches[0];
    swipe = {
      id: slip.id,
      x: touch.clientX,
      y: touch.clientY,
      decided: false,
      dx: 0,
    };
  }

  function touchMove(event, slip) {
    const start = swipe;
    if (!start || start.id !== slip.id || event.touches.length !== 1) return;
    const touch = event.touches[0];
    const dx = touch.clientX - start.x;
    const dy = touch.clientY - start.y;
    if (!start.decided) {
      if (Math.abs(dx) < SWIPE_SLOP && Math.abs(dy) < SWIPE_SLOP) return;
      // 竖向优先：这一下是滚动，彻底放弃手势，列表照常跟随手指。
      if (Math.abs(dy) >= Math.abs(dx)) {
        swipe = null;
        return;
      }
      start.decided = true;
    }
    // 已经判为横向：拦下浏览器可能的横向平移，并且只让右滑跟手（`swipeOffset` 夹上限）。
    event.preventDefault();
    start.dx = swipeOffset(dx);
  }

  function touchEnd(slip) {
    const start = swipe;
    // 先清状态：无论这一下触发不触发，行都要回弹到原位（CSS 过渡负责平滑）。
    swipe = null;
    if (!start || start.id !== slip.id || !start.decided) return;
    if (swipeIntent({ dx: start.dx, dy: 0 }) === "complete") {
      onswipecomplete?.(slip);
    }
  }

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
   * 当前这次编辑的**不可变快照**：`{ id, original }`。
   *
   * 为什么不能直接读 `editingId` / `draft`：用户改着 A 又去双击 B 时，这两个值
   * 会被立刻改写成 B 的；而 A 那次提交是异步的，回来时读到的已经是别人的状态。
   * 实测后果是**改动被静默丢弃**（A 的内容没保存，B 也没进入编辑）。
   * 提交必须针对"这一次编辑"，而不是"当时碰巧在编辑哪一条"。
   */
  let session = $state(null);

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
   * 正在编辑的条目**消失了**（外部修改后重新载入、隔离重建）时必须退出编辑态。
   *
   * 否则 `editingId` 永远非空 → 双向绑定的 `editing` 恒为 true → App 里
   * `if (listEditing) return` 会把 `Ctrl+Z` **永久拦掉**，而界面上根本没有编辑框，
   * 用户只看到撤销毫无反应。实测过这个卡死：外部替换文件 + 重新载入之后再按 Ctrl+Z，
   * 什么都不会发生。
   */
  $effect(() => {
    if (editingId !== null && !slips.some((slip) => slip.id === editingId)) {
      editingId = null;
      draft = "";
      session = null;
      composing = false;
      cancelled = false;
    }
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

  async function beginEdit(slip) {
    // 锁定只保护正文；这里要明确告诉用户为什么双击没反应，否则像是坏了。
    if (slip.locked) {
      onnotice?.("已锁定，先解锁才能修改");
      return;
    }
    // 切到另一条之前，先把上一条**按它自己的会话**提交掉，否则改动会丢。
    // 必须等提交结果：失败时留在当前这条，不能让用户刚打的字被新目标顶掉。
    if (session && session.id !== slip.id) {
      if (!(await commitEdit())) return;
    }
    editingId = slip.id;
    draft = slip.text;
    session = { id: slip.id, original: slip.text };
    composing = false;
    cancelled = false;
  }

  /** 取消不产生任何请求：编辑框里的内容不是用户想保存的东西。 */
  function cancelEdit() {
    cancelled = true;
    composing = false;
    editingId = null;
    draft = "";
    session = null;
  }

  /**
   * 提交当前编辑。
   *
   * 返回是否"可以离开这次编辑"：没改、已取消、后端接受 → true；后端拒绝或请求
   * 抛错 → false。调用方（Tab 切下一条、Enter 交还焦点、beginEdit 切目标）都靠
   * 这个返回值决定要不要继续，否则失败时又会切走并覆盖草稿。
   *
   * @returns {Promise<boolean>}
   */
  async function commitEdit() {
    // 失焦可能和 Enter 撞在一起；也承接"取消后立刻失焦"的情况。这几种都没有
    // 这一次需要保存的东西：已取消/没会话按成功处理；已在提交中则等它自己的调用方
    // 收尾，这里按 true 放行，免得同一次草稿被发第二遍。
    const current = session;
    if (cancelled || committing || !current) {
      return true;
    }
    // 没改就不用发请求，免得平白把提示区刷成"已修改"。
    if (draft === current.original) {
      finishSession(current.id);
      return true;
    }
    let accepted = false;
    committing = true;
    try {
      accepted =
        (await onedit?.({ id: current.id, text: current.original }, draft)) ??
        false;
    } catch {
      // 上层 `handleEdit` 自己会兜住错误并返回 false；这里再兜一层，
      // 保证 `commitEdit` 永不 reject，调用方可以安全地 `void` 它。
      accepted = false;
    } finally {
      committing = false;
    }
    // 失败时保持编辑态与草稿：用户的输入不能丢。
    if (accepted) {
      finishSession(current.id);
      return true;
    }
    return false;
  }

  /**
   * 收尾：只有这条**仍然是当前编辑目标**时才清界面状态。
   *
   * 用户可能已经切到别的行了——那时清空会把新展开的编辑框一起关掉（实测过）。
   */
  function finishSession(id) {
    if (session?.id !== id) {
      return;
    }
    session = null;
    if (editingId === id) {
      editingId = null;
      draft = "";
    }
  }

  async function handleKeydown(event) {
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
      // 免得把用户刚点去的输入框抢回来。失败时**不**交还焦点：编辑框还在原地，
      // 焦点跳回行上等于把用户从还没保存的草稿边赶走。
      if (await commitEdit()) await refocusRow(id);
    } else if (event.key === "Escape") {
      event.preventDefault();
      const id = editingId;
      cancelEdit();
      await refocusRow(id);
    } else if (event.key === "Tab") {
      /*
       * 编辑态里的 Tab 不该把焦点甩到浏览器别处：它的含义是「保存这条，接着改下一条」。
       * 先提交再切——顺序反了会丢当前改动（Tab 移动焦点会触发失焦提交，而那时会话已是新行）。
       */
      event.preventDefault();
      const id = editingId;
      const index = slips.findIndex((item) => item.id === id);
      const target = tabTarget(index, slips.length, event.shiftKey);
      if (draft.trim().length === 0) {
        onnotice?.("纸条内容不能为空");
        return;
      }
      if (target === null) {
        // 已在首/末条：提交后把焦点还给当前行，不跳到不存在的位置。
        if (await commitEdit()) await refocusRow(id);
        return;
      }
      // 先等提交结果再切目标：提交失败时留在这一条，草稿不会被下一条顶掉。
      // （`beginEdit` 自己也会提交一次，但那时 `session` 已是新目标——
      // 顺序反了就会把同一份草稿发两遍。）
      if (!(await commitEdit())) return;
      await beginEdit(slips[target]);
    }
  }

  /**
   * 失焦提交。
   *
   * 必须带上"这一行是谁"：切换编辑目标时，旧输入框会因卸载而失焦，而那时
   * `session` 已经是新那一条的了。不校验就会拿别人的会话做判断——实测的后果是
   * 旧改动被静默丢弃、新展开的编辑框也被关掉。
   */
  function handleBlur(slipId) {
    if (!session || session.id !== slipId) {
      return;
    }
    if (cancelled) {
      cancelled = false;
      return;
    }
    // 把内容删空后直接点外部失焦：视为放弃修改并恢复原文，避免卡在失焦的空编辑框里。
    if (draft.trim().length === 0) {
      cancelEdit();
      return;
    }
    void commitEdit();
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
      case "PageDown":
      case "PageUp":
      case "Home":
      case "End": {
        // 一页 5 行、Home/End 到首尾；目标索引由纯函数算，边界（空列表、末页不足 5 行）
        // 在 keys.js 里已被断言覆盖。
        const target = listNavTarget(index, itemEls.length, event.key);
        if (target === null) {
          return;
        }
        event.preventDefault();
        itemEls[target]?.focus();
        return;
      }
      case " ":
        // 空格在 span 上默认是滚动，必须挡掉，否则勾选的同时页面会跳。
        event.preventDefault();
        ontoggle?.(slip);
        return;
      case "Enter":
        event.preventDefault();
        void beginEdit(slip);
        return;
      case "p":
      case "P":
        /*
         * P = 置顶（R95）。用字母键而不是再拼一个 Ctrl 组合：列表项本身就是
         * `role="textbox"` 的只读焦点位，单键在这里不会与输入冲突。
         * 大写也认（按住 Shift 时 `event.key` 是 "P"），否则键盘用户得先确认大小写。
         */
        event.preventDefault();
        onpin?.(slip);
        return;
      default:
        return;
    }
  }
</script>

<ul class="slips">
  {#if slips.length === 0 && archivedView}
    <li class="slips__empty">
      <p class="slips__empty-title">还没有归档</p>
      <p class="slips__empty-foot">
        归档只是从列表收起，内容仍然在 inbox.md 里
      </p>
    </li>
  {:else if slips.length === 0 && archivedCount > 0}
    <!--
      列表空了但归档里还有东西：这时候说"还没有内容"是**错误信息**——用户刚把一条
      归档掉，内容还在 inbox.md 里。引导文案（按热键/回车收录）对他也毫无用处。
    -->
    <li class="slips__empty">
      <p class="slips__empty-title">都归档了</p>
      <p class="slips__empty-foot">
        {archivedCount} 条已完成的内容在归档里，点右上角「已归档 {archivedCount}」可以翻回来
      </p>
    </li>
  {:else if slips.length === 0 && filtering}
    <!--
      "过滤没匹配到"和"真的没有内容"必须分开说。给过滤空结果看首次使用引导，
      用户会以为数据没了——这比没有空态更糟。
    -->
    <li class="slips__empty">
      <p class="slips__empty-title">没有匹配的纸条</p>
      <p class="slips__empty-foot">按 <kbd>Esc</kbd> 退出过滤</p>
    </li>
  {:else if slips.length === 0}
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
      <!-- 标签只算一次：模板里两处要用（判空 + 遍历），重复调用纯属浪费。 -->
      {@const tags = extractTags(slip.text)}
      <li
        class:slip--done={slip.done}
        class="slip"
        class:slip--swiping={swipe?.id === slip.id}
        style:transform={swipe?.id === slip.id
          ? `translateX(${swipe.dx}px)`
          : null}
        ontouchstart={(event) => touchStart(event, slip)}
        ontouchmove={(event) => touchMove(event, slip)}
        ontouchend={() => touchEnd(slip)}
        ontouchcancel={() => (swipe = null)}
        ondblclick={() => {
          // 双击整行都能进编辑——原来只有点在文字上才有效，行内空白处点不动。
          // 复选框与操作按钮各自阻断冒泡（在按钮上双击的语义是"点了两下按钮"）。
          //
          // **锁定判断交给 beginEdit**：那里会给"已锁定，先解锁才能修改"的提示。
          // 我一开始在这里 `if (slip.locked) return;` 短路，结果把提示一起跳过了——
          // 端到端断言"锁定后双击给出原因"当场抓到（R46 引入的回归）。
          void beginEdit(slip);
        }}
      >
        <button
          type="button"
          class="slip__box"
          class:slip__box--done={slip.done}
          aria-checked={slip.done}
          role="checkbox"
          aria-label={slip.done ? "标记为未完成" : "标记为完成"}
          onclick={() => ontoggle?.(slip)}
          ondblclick={(event) => event.stopPropagation()}
        ></button>

        {#if editingId === slip.id}
          <input
            class="slip__edit"
            type="text"
            bind:value={draft}
            use:focusAndSelect
            onkeydown={handleKeydown}
            onblur={() => handleBlur(slip.id)}
            oncompositionstart={() => (composing = true)}
            oncompositionend={() => (composing = false)}
            aria-label="编辑纸条"
          />
        {:else}
          <!--
            正文与徽章分成两行（`.slip__body`）：徽章是**渲染层的另一段 DOM**，
            不塞进 `.slip__text` 里。两条理由：
              ① 正文的派生视图必须与 `slip.text` 逐字节一致——徽章文本会把
                 `textContent` 变成"原文 + #标签"，任何以它为准的校验都会失真；
              ② 标签多的时候需要整行折行，塞在正文行里会把文字挤成窄条。
          -->
          <div class="slip__body">
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
              onkeydown={(event) => handleItemKeydown(event, slip, index)}
              title={slip.locked ? "已锁定，先解锁才能修改" : "双击编辑"}
            >
              <!--
                逐段渲染而不是 `{@html}`：正文来自 inbox.md，这个文件用户随时可编辑，
                当 HTML 插进 DOM 就等于给它一个执行入口。这里的每一段要么是纯文本
                （Svelte 自动转义），要么是下面这几种已知标签。

                `buildInline` 只做视图加工（混排微间距、命中高亮），**不改字**：
                所有非 gap 片段的文本拼起来逐字节等于 `slip.text`。
              -->
              {#each buildInline(slip.text, parsedFilter) as token}
                {#if token.type === "bold"}
                  <strong
                    >{#if token.hit}<mark class="slip__highlight"
                        >{token.value}</mark
                      >{:else}{token.value}{/if}</strong
                  >
                {:else if token.type === "code"}
                  <code
                    >{#if token.hit}<mark class="slip__highlight"
                        >{token.value}</mark
                      >{:else}{token.value}{/if}</code
                  >
                {:else if token.type === "link"}
                  <a
                    class="slip__link"
                    href={token.href}
                    onclick={(event) => {
                      // 交给系统浏览器；双击链接不该顺带进入编辑。
                      event.preventDefault();
                      event.stopPropagation();
                      ondelink?.(token.href);
                    }}
                    ondblclick={(event) => event.stopPropagation()}
                    >{token.value}</a
                  >
                {:else if token.type === "gap"}
                  <!--
                    中英混排的微间距（R87）：**零宽元素**，不是空格字符。
                    正文里塞一个真实空格会改写用户字节；这里只是让"汉字紧跟 abc"
                    看起来不至于挤在一起。`aria-hidden` 是必须的——它没有语义。
                  -->
                  <span class="slip__gap" aria-hidden="true"></span>
                {:else if token.hit}
                  <!-- 过滤命中：`<mark>` 只是视觉强调，文本仍逐字相同。 -->
                  <mark class="slip__highlight">{token.value}</mark>
                {:else}{token.value}{/if}
              {/each}
            </span>
            {#if tags.length > 0}
              <span class="slip__tags"
                >{#each tags as tag (tag)}<!--
                    标签徽章（R84）：正文里的 `#工作` 原样留着，这里只是额外的可扫视入口。
                  --><span
                    class="slip__tag">#{tag}</span
                  >{/each}</span
              >
            {/if}
          </div>
          {#if archivedView}
            <button
              type="button"
              class="slip__archive"
              aria-label="恢复纸条"
              title="恢复到列表"
              onclick={() => onunarchive?.(slip)}
              ondblclick={(event) => event.stopPropagation()}
            >
              <svg
                viewBox="0 0 16 16"
                width="13"
                height="13"
                aria-hidden="true"
              >
                <path
                  d="M3 8a5 5 0 1 1 1.6 3.7M3 4.5V8h3.5"
                  fill="none"
                  stroke="currentColor"
                  stroke-width="1.4"
                  stroke-linecap="round"
                  stroke-linejoin="round"
                />
              </svg>
            </button>
          {:else if slip.done}
            <button
              type="button"
              class="slip__archive"
              aria-label="归档纸条"
              title="归档（从列表收起）"
              onclick={() => onarchive?.(slip)}
              ondblclick={(event) => event.stopPropagation()}
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
          {#if archivedView}
            <!--
              R104：彻底删除。**不可逆**，所以二次确认由上层闸门负责（4 秒自动还原），
              这里只做两件事：低对比地露出来，以及把"已武装"状态标出去——
              否则用户根本看不出自己刚点的第一下是在确认什么。
            -->
            <button
              type="button"
              class="slip__purge"
              class:slip__purge--armed={purgeArmedId === slip.id}
              aria-label="彻底删除（不可恢复）"
              title={purgeArmedId === slip.id
                ? "再点一次确认：彻底删除后无法恢复（已留备份）"
                : "彻底删除（不可恢复）"}
              onclick={() => onpurge?.(slip)}
              ondblclick={(event) => event.stopPropagation()}
            >
              <svg
                viewBox="0 0 16 16"
                width="13"
                height="13"
                aria-hidden="true"
              >
                <path
                  d="M3 4.5h10M6.5 4.5V3h3v1.5M4.5 4.5l.6 8.2a1 1 0 0 0 1 .8h4.8a1 1 0 0 0 1-.8l.6-8.2"
                  fill="none"
                  stroke="currentColor"
                  stroke-width="1.3"
                  stroke-linecap="round"
                  stroke-linejoin="round"
                />
              </svg>
            </button>
          {/if}
          <button
            type="button"
            class="slip__pin"
            class:slip__pin--on={slip.pinned}
            aria-pressed={slip.pinned}
            aria-label={slip.pinned ? "取消置顶" : "置顶纸条"}
            title={slip.pinned ? "取消置顶（P）" : "置顶到最前（P）"}
            onclick={() => onpin?.(slip)}
            ondblclick={(event) => event.stopPropagation()}
          >
            <svg viewBox="0 0 16 16" width="13" height="13" aria-hidden="true">
              <path
                d="M6 2.75h4l-.6 3.1 2.1 2.4h-7l2.1-2.4-.6-3.1zM8 8.25v5"
                fill="none"
                stroke="currentColor"
                stroke-width="1.35"
                stroke-linejoin="round"
                stroke-linecap="round"
              />
            </svg>
          </button>
          <button
            type="button"
            class="slip__lock"
            class:slip__lock--on={slip.locked}
            aria-pressed={slip.locked}
            aria-label={slip.locked ? "解锁纸条" : "锁定纸条"}
            title={slip.locked ? "解锁正文" : "锁定正文"}
            onclick={() => onlock?.(slip)}
            ondblclick={(event) => event.stopPropagation()}
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
          {#if !editingId}
            <!--
              相对时间不用定时器轮询：它在渲染时按当前时刻算一次就够了。
              悬浮给精确时间，需要准确值时看那里。
            -->
            <time
              class="slip__time"
              datetime={slip.createdAt}
              title={exactLocal(slip.createdAt)}
              >{relativeTime(slip.createdAt)}</time
            >
          {/if}
        {/if}
      </li>
    {/each}
  {/if}
</ul>
