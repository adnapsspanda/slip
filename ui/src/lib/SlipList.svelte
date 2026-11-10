<script>
  /**
   * 纸片列表。
   *
   * 只负责展示与转发点击：任何"点完之后应该显示什么"都由后端快照决定，
   * 这里不做乐观更新，否则就要回答"发射丢失时以谁为准"。
   */
  let { slips = [], ontoggle } = $props();
</script>

<ul class="slips">
  {#if slips.length === 0}
    <li class="slips__empty">还没有内容。先写一条试试。</li>
  {:else}
    {#each slips as slip (slip.id)}
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
        <span class="slip__text">{slip.text}</span>
      </li>
    {/each}
  {/if}
</ul>
