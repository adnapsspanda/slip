# Ralph 式迭代待办（本文件的唯一职责：驱动循环）

`ralph` 不是本仓库或本 harness 的工具。这里用**持久目标 + 自动续轮**实现同一件事：
每轮从下面的清单取一条，做完、验完、提交，再记一行。

## 每轮的固定流程

1. 取**价值最高**的未完成项（编号只作引用，不决定顺序）；发现更值当的问题可插队，但要写进本文件。
2. **先读代码再下结论。** 见铁律第 4 条。
3. 实现。改动范围以该项的「验收标准」为边界，不顺手扩张。
4. 跑完整门禁，**全绿才提交**：
   ```powershell
   Push-Location ui; & "C:\Users\spand\.bun\bin\bun.exe" run build; Pop-Location
   cargo fmt --all --check
   cargo clippy --workspace --all-targets --locked -- -D warnings
   cargo test --workspace --locked
   pwsh -File tools/verify-hotkey.ps1
   ```
   注意 `ui/dist` 是编译期依赖（`tauri::generate_context!`），**前端构建必须排在
   任何 cargo 命令之前**，否则 `build.rs` 会直接报缺产物。
5. 一项一个提交。提交信息说明「为什么」，不复述 diff。
6. 回来把该项标成 ✅，附上验证证据（数字/日志摘要/行号）。

## 铁律（违反即回滚）

1. **不重新引入**：watcher、内容去重、回调/扩展点、事件总线、`PostCommit`、
   `ReadOnly` 作为文件状态、物理归档、v1 内 SQLite、CLI/MCP 转发、`state_rev`、
   跨字段/时钟合法性检查、整份文档重新序列化。这份清单见 `AGENTS.md`。
2. **不为了凑轮数而改**。50 轮是上限。没活干就停，并说明为什么。
3. **文档承诺必须与实现一致**。任何"文档说有、代码没有"的矛盾优先于新功能。
4. **不得凭记忆断言代码状态。** 任何"某函数没有做 X"的结论，必须本轮现场读代码
   确认，并在记录里附上文件与行号。`R-01` 就是反例：凭归档功能落地前的印象下了
   结论，白花一轮——代码里 `Patch::Unarchive`、`apply_patch` 分支、`inverse_batch`
   映射、以及 `pipeline.rs:301` 的端到端测试全都在。

## 待办

| ID | 事项 | 证据 | 验收标准 | 状态 |
|---|---|---|---|---|
| R-02 | 死视图路径（**需要产品决策，勿擅自删**） | 整个 `ui/` 零引用 `toggleCollapse` / `DragEnd` / `DockSideChanged` / `CollapseToggled`；后端仍保留 `ViewEvent` 四变体、`Command::ToggleCollapse`、`PaperViewSet.collapsed/dock_side`，以及**只为它们服务**的 `PREFS_DEBOUNCE`（仅 `dispatch.rs:400`、`:418` 触发） | 二选一并在 `docs/design` 落定：① 接线（需要先定义 UI 入口）；② 整体删除并把 `prefs` 收窄，同时保证旧 `ui-state.json` 仍可读（serde default）。删除可连带简化 `dispatch` 的去抖机制 | 待决策 |
| R-06 | `csp: null` | `tauri.conf.json` 未设 CSP。当前无远程内容、无 `{@html}`，风险低但无纵深 | 补最小 CSP（`default-src 'self'`），确认 Svelte 内联样式与 WebView2 下仍正常：44 项验收全过 | 待办 |
| R-07 | 前端无 lint/format | 只有 `vite build`；格式靠人。仓库其余部分都有门禁 | 引入 `prettier --check` 并接进 `.githooks/pre-commit` 与 CI；**不**引入 eslint 规则集（避免新噪音面） | 待办 |
| R-08 | `.ps1` 必须带 BOM | `edit`/`write` 工具会剥掉 BOM，导致 `verify-hotkey.ps1` 被按 GBK 解码而报语法错，本次会话撞了 4 次 | 在 `.githooks/pre-commit` 里检查 `*.ps1` 是否以 `EF BB BF` 开头，缺失则拒绝并提示一句修法 | 待办 |
| R-05 | 限制常量前后端各一份 | `slip-domain` 的 `MAX_TEXT_CODEPOINTS=2000` / `MAX_LINES=100` 与 `Capture.svelte` 的字面量重复，无单一来源 | 至少加双向注释互指；更理想是加一条断言两处相等的契约测试（前端值可在构建期注入） | 待办 |
| R-09 | 无安装包 | `bundle.active: false`，只能分发绿色 exe | 确认是否刻意；若是，在 README「已知限制」里写明不提供安装包及原因 | 待办 |
| R-03 | 摆放兜底居中到主屏 | **降级**：原证据（用户两次启动坐标恒为 `(965,344)`）实际来自沙盒启动，交互桌面上 `place_near_cursor` 正常跟随光标 | 仅在能复现"真实桌面下光标不可读"时再修；修法是把兜底顺序改为 光标所在屏 → 前台窗口所在屏 → 主屏 | 降级 |
| R-01 | ~~归档撤销承诺与实现不符~~ | **撤回**：`Patch::Unarchive`（`model.rs:52`）、`apply_patch` 分支（`state.rs:135`）、`inverse_batch` 双向映射（`dispatch.rs:565-568`）、领域测试（`state.rs:304`）、端到端测试（`pipeline.rs:301` `archive_and_undo_preserve_done_state_through_disk`）全部就位 | 无需改动。文档承诺与实现**一致** | ✅ 撤回 |

## 完成记录

- **R-04｜4469d04 之后的文档提交**：设计文档 §9.7 补修订表第 14 行与 §9.7.2，
  说明 `spike_*` 命令集已删、自检迁到 `diag.rs`（`SLIP_LOG=debug`），并把
  §9.7.1 人眼清单第 3 条里对 `spike_hide` 的依赖改成 `Esc`/点窗口外。
- **R-01｜撤回**：上一轮把它当 P0 报出来，实际代码完整实现了归档撤销且有端到端
  覆盖。教训写进铁律第 4 条。
