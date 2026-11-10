# Ralph 式迭代待办（本文件的唯一职责：驱动循环）

`ralph` 不是本仓库或本 harness 的工具。这里用**持久目标 + 自动续轮**实现同一件事：
每轮从下面的清单取一条，做完、验完、提交，再记一行。

## 每轮的固定流程

1. 取**编号最小**的未完成项；若发现更值当的问题可以插队，但要写进本文件。
2. 实现。改动范围以该项的「验收标准」为边界，不顺手扩张。
3. 跑完整门禁，**全绿才提交**：
   ```powershell
   cargo fmt --all --check
   cargo clippy --workspace --all-targets --locked -- -D warnings
   cargo test --workspace --locked
   Push-Location ui; & "C:\Users\spand\.bun\bin\bun.exe" run build; Pop-Location
   pwsh -File tools/verify-hotkey.ps1
   ```
   注意 `ui/dist` 是编译期依赖（`tauri::generate_context!`），**前端构建必须排在
   任何 cargo 命令之前**，否则 `build.rs` 会直接报缺产物。
4. 一项一个提交。提交信息说明「为什么」，不复述 diff。
5. 回来把该项标成 ✅，附上验证证据（数字/日志摘要）。

## 铁律（违反即回滚）

- **不重新引入**：watcher、内容去重、回调/扩展点、事件总线、`PostCommit`、
  `ReadOnly` 作为文件状态、物理归档、v1 内 SQLite、CLI/MCP 转发、`state_rev`、
  跨字段/时钟合法性检查、整份文档重新序列化。这份清单见 `AGENTS.md`。
- **不为了凑轮数而改**。50 轮是上限。没活干就停，并说明为什么。
- **文档承诺必须与实现一致**。任何"文档说有、代码没有"的矛盾优先于新功能。

## 待办

| ID | 事项 | 证据 | 验收标准 | 状态 |
|---|---|---|---|---|
| R-01 | 归档撤销的承诺与实现不符 | `README` 与 `lib.rs::archive` 注释都声称「归档进撤销栈，Ctrl+Z 整批恢复」；但 `dispatch.rs::undoable()` 只接受 `SetText`/`SetDone`，`inverse_batch` 遇 `Patch::Archive` 直接 `return None` | 二选一：① 新增 `Patch::Unarchive`（或 `Archive { at: Option<_> }`）并让归档批次进撤销栈；② 改掉 README 与注释，明确「归档不可撤销」。**先在 `docs/design` 里落定方向再动代码** | 待办 |
| R-02 | 死视图路径 | 前端零处引用 `toggleCollapse` / `DragEnd` / `DockSideChanged` / `CollapseToggled`；`bridge.js` 无对应导出。但后端保留了 `Command::ToggleCollapse`、三个 `ViewEvent` 分支、`prefs.collapsed/dock_side`，以及**只为它们服务**的 `PREFS_DEBOUNCE` 去抖机制 | 要么接线（需要在设计文档里补 UI 入口的定义），要么整体删除并把 `prefs` 收窄。删除要保证旧 `ui-state.json` 仍可读（serde default） | 待办 |
| R-03 | 摆放兜底居中的是主屏 | `place_near_cursor` 拿不到光标/显示器时调 `window.center()`，Tauri 居中到**主屏**。真实证据：用户两次启动的窗口坐标恒为物理 `(965,344)`（= 虚拟 `(643,229)`，主屏正中），而交互桌面上启动会跟随光标 | 兜底顺序改为：光标所在显示器 → 前台窗口所在显示器 → 主屏。加一条可测的单元或用 `diag` 观察启动日志 | 待办 |
| R-04 | 设计文档残留 9 处 `spike` | `spike.rs` 已删除，但 `docs/design` 仍把 §9.7 窗口 spike 当计划描述 | 在 §9.7 补一段修订说明：调研已结束，命令集已删，结论并入 `src-tauri/src/diag.rs` | 待办 |
| R-05 | 限制常量前后端各一份 | `slip-domain` 的 `MAX_TEXT_CODEPOINTS=2000` / `MAX_LINES=100` 与 `Capture.svelte` 的字面量重复，无单一来源 | 至少加双向注释互指；理想是让前端从一份生成物取值，或加契约测试断言两处相等 | 待办 |
| R-06 | `csp: null` | `tauri.conf.json` 未设 CSP。当前无远程内容、无 `{@html}`，风险低 | 补最小 CSP（`default-src 'self'`），确认 Svelte 内联样式与 WebView2 下仍正常（跑一遍 44 项验收） | 待办 |
| R-07 | 前端无 lint/format | 只有 `vite build`；没有 prettier/eslint，格式靠人 | 引入 `prettier --check` 并接进 `.githooks/pre-commit` 与 CI；不引入 eslint 规则集（避免新噪音面） | 待办 |
| R-08 | `.ps1` 必须带 BOM | `edit`/`write` 工具会剥掉 BOM，导致 `verify-hotkey.ps1` 被按 GBK 解码而报语法错 | 让脚本自身容忍无 BOM，或在钩子里检查并提示。至少要有一句明确的失败提示 | 待办 |
| R-09 | 无安装包 | `bundle.active: false`，只能分发绿色 exe | 确认是否刻意；若是，在 README 写明「不提供安装包」及原因 | 待办 |

## 完成记录

（每完成一项在此追加：ID、提交、验证证据）
