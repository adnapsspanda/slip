<script>
  /**
   * 捕获框。
   *
   * 关于回车有两条独立规则，缺一条都会伤到用户：
   * - `Shift+Enter` 是换行，永远不提交；
   * - 输入法正在选词时（`composing` 或 `event.isComposing`）的回车只是确认
   *   候选词，**绝不能**把半截拼音收进 Inbox。
   */
  let { hint = "", disabled = false, onsubmit, onescape } = $props();

  let text = $state("");
  /** 每次编辑递增；随请求带上，ACK 回来时用来判断草稿是否还是提交时的那一份。 */
  let draftVersion = $state(0);
  let composing = $state(false);
  let field = $state(null);

  export function focus() {
    field?.focus();
  }

  export function clear() {
    text = "";
    draftVersion += 1;
  }

  export function getDraftVersion() {
    return draftVersion;
  }

  function input(event) {
    text = event.currentTarget.value;
    draftVersion += 1;
  }

  function keydown(event) {
    // Escape 交给上层收窗口，但在输入法组合态里它是"取消候选词"的意思：
    // 这时放行给 IME，否则用户想撤销一次拼音输入却把整张纸片收走了。
    // `keyCode === 229` 是 WebView2 上某些 IME 的兜底信号，`isComposing` 未必置位。
    if (event.key === "Escape") {
      if (composing || event.isComposing || event.keyCode === 229) {
        return;
      }
      event.preventDefault();
      onescape?.();
      return;
    }

    if (event.key !== "Enter" || event.shiftKey) {
      return;
    }
    if (composing || event.isComposing || event.keyCode === 229) {
      return;
    }
    event.preventDefault();
    if (text.trim().length === 0) {
      return;
    }
    onsubmit?.(text);
  }
</script>

<textarea
  bind:this={field}
  value={text}
  class="capture__input"
  rows="2"
  placeholder={disabled ? "后端不可用" : "想到什么就打在这里，回车收进 Inbox"}
  spellcheck="false"
  {disabled}
  onkeydown={keydown}
  oninput={input}
  oncompositionstart={() => (composing = true)}
  oncompositionend={() => (composing = false)}
></textarea>
<div class="capture__hint">{hint}</div>
