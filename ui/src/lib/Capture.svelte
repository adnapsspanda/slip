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

  /** 与 slip_domain::text / slip_domain::split 保持同口径。 */
  const MAX_CODEPOINTS = 2000;
  const MAX_LINES = 100;

  let text = $state("");
  /** 每次编辑递增；随请求带上，ACK 回来时用来判断草稿是否还是提交时的那一份。 */
  let draftVersion = $state(0);
  let composing = $state(false);
  let field = $state(null);

  /** JS 的 `[...s]` 与 Rust 的 `s.chars()` 都按码点迭代，单位一致。 */
  const codepointLen = (s) => [...s].length;

  /**
   * 本地提示，**不是裁决**。真正的拒绝仍由领域层给出。
   *
   * 在这里提前拦的理由：后端的 `TextTooLong` 只说"太长"，说不清是第几行、
   * 超出多少；多行粘贴时用户根本不知道该改哪一行。
   */
  let problem = $derived.by(() => {
    const lines = text
      .split(/\r\n|\r|\n/)
      .map((line) => line.trim())
      .filter((line) => line.length > 0);
    if (lines.length > MAX_LINES) {
      return `一次最多收 ${MAX_LINES} 行，现在是 ${lines.length} 行`;
    }
    for (let i = 0; i < lines.length; i += 1) {
      const n = codepointLen(lines[i]);
      if (n > MAX_CODEPOINTS) {
        return `第 ${i + 1} 行 ${n} 字，单条上限 ${MAX_CODEPOINTS}`;
      }
    }
    return "";
  });

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
    if (problem) {
      return;
    }
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
<div class="capture__hint" class:capture__hint--warn={problem !== ""}>
  {problem || hint}
</div>
