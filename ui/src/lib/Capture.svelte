<script>
  import { stripListPrefix, cleanPastedText } from "./inline.js";
  import { submitIntent } from "./keys.js";
  /**
   * 捕获框。
   *
   * 关于回车有两条独立规则，缺一条都会伤到用户：
   * - `Shift+Enter` 是换行，永远不提交；
   * - 输入法正在选词时（`composing` 或 `event.isComposing`）的回车只是确认
   *   候选词，**绝不能**把半截拼音收进 Inbox。
   */
  let {
    hint = "",
    disabled = false,
    onsubmit,
    onescape,
    onseeklist,
  } = $props();

  /**
   * 与领域层同值，且由 `crates/slip-domain/tests/frontend_contract.rs` 钉住：
   * `MAX_CODEPOINTS` = `slip_domain::MAX_TEXT_CODEPOINTS`，
   * `MAX_LINES` = `slip_domain::MAX_LINES`。改任何一边，那条测试都会红。
   *
   * 裁决点仍然只在领域层：这里只用来提前提示"第几行超了多少"，不作数。
   */
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
   * 上限预检。**它确实会拦住提交**（见 `keydown` 里的 `if (problem) return`），
   * 所以它的口径必须与领域层完全一致，否则就是"合法输入被界面拒收"。
   *
   * 提前拦的理由：后端的 `TextTooLong` 只说"太长"，说不清是第几行、超出多少；
   * 多行粘贴时用户根本不知道该改哪一行。
   *
   * 因此这里先做与 `split_drafts` 相同的归一化（剥列表前缀 + trim）再计数。
   */
  let problem = $derived.by(() => {
    // 必须**先剥列表前缀再量**：领域层就是这么做的。否则 `- ` + 2000 字的合法输入
    // 会被这里误判成 2002 字并拦住提交（实测过这个 bug）。
    const lines = text
      .split(/\r\n|\r|\n/)
      .map((line) => stripListPrefix(line).trim())
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

  /**
   * 粘贴时清洗列表前缀。
   *
   * 为什么在**粘贴**这一刻做、而不是提交时做：用户从别处复制三条待办粘进来，应当立刻看见
   * "它已经变成三条干净的行"，而不是等到落盘后才发现前缀没了。
   *
   * 只在真的剥掉了前缀时才 `preventDefault`——普通粘贴必须走默认行为，
   * 否则会破坏撤销栈、也会丢掉富文本粘贴里我们没处理的部分。
   */
  function paste(event) {
    const raw = event.clipboardData?.getData("text");
    const { text: cleaned, changed } = cleanPastedText(raw ?? "");
    if (!changed) {
      return;
    }
    event.preventDefault();
    const field = event.currentTarget;
    const start = field.selectionStart ?? text.length;
    const end = field.selectionEnd ?? text.length;
    const next = text.slice(0, start) + cleaned + text.slice(end);
    text = next;
    draftVersion += 1;
    // 光标落到刚插入内容的末尾。
    queueMicrotask(() => {
      const caret = start + cleaned.length;
      field.setSelectionRange(caret, caret);
    });
    hint = cleaned.includes("\n")
      ? "已去掉列表前缀，并拆成多行"
      : "已去掉列表前缀";
    setTimeout(() => {
      if (hint === "已去掉列表前缀" || hint === "已去掉列表前缀，并拆成多行") {
        hint = "";
      }
    }, 2000);
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

    const intent = submitIntent(event);
    // 输入法组合期：回车是"确认候选词"，Shift/Ctrl 都不改变这个含义，一律放行给 IME。
    if (intent === "composing") {
      return;
    }
    if (intent !== "submit") {
      // 换行（Shift+Enter）交给 textarea 自己处理，这里不拦。
      if (intent === "newline") {
        return;
      }
      // 输入框为空时按 ↓ 直接进入列表：让"录入 → 浏览"不必摸鼠标。
      // 有内容时不劫持 ↓——那时用户多半就是想移动光标。
      if (event.key === "ArrowDown" && text.trim().length === 0) {
        event.preventDefault();
        onseeklist?.();
      }
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
  onpaste={paste}
  oninput={input}
  oncompositionstart={() => (composing = true)}
  oncompositionend={() => (composing = false)}></textarea>
<div class="capture__hint" class:capture__hint--warn={problem !== ""}>
  {problem || hint}
</div>
