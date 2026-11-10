# 阶段一体检报告（R01–R10）

> 生成于本轮验收，数据全部来自实际执行，不是估计值。

## 门禁基线

| 门 | 命令 | 结果 |
|---|---|---|
| 前端构建 | `bun run build` | 通过 |
| 前端格式 | `bun run format:check` | `All matched files use Prettier code style!` |
| 编码 | `*.ps1` 必须以 `EF BB BF` 开头 | 通过 |
| 脚本语法 | `tools/check-ps1-syntax.ps1` | 全部通过 |
| 格式 | `cargo fmt --all --check` | 通过 |
| 静态检查 | `cargo clippy --workspace --all-targets -- -D warnings` | 通过 |
| 单元/集成测试 | `cargo test --workspace` | **204 项通过**（另有 4 项性能测试 `#[ignore]`） |
| 端到端 | `tools/verify-hotkey.ps1` | **46 项通过 / 0 失败 / 1 跳过** |

`1 跳过` = 负坐标副屏场景（本机当前只有一块显示器）。该路径的几何已由
`src-tauri/src/hotkey.rs` 的单元测试 `handles_monitor_with_negative_origin` 覆盖。

## 规模

| 项 | 数值 |
|---|---|
| 提交数 | 22 |
| Rust 行数 | 8365 |
| 前端行数（svelte/js/css） | 1380 |
| IPC 命令数 | 14 |
| webview 权限 | 1 项（`core:default`） |
| 提交钩子步骤 | 7 |
| CI 步骤 | 11 |

## 架构体检（逐条核实，非推断）

| 不变量 | 结论 | 依据 |
|---|---|---|
| 依赖单向 `src-tauri → slip-app → slip-store → slip-domain` | 成立 | `slip-app/Cargo.toml` 无 `tauri` 依赖 |
| `slip-domain` 零 I/O、零平台依赖 | 成立 | `src/` 下无 `std::fs`/`std::process` 使用（仅有文档注释提到） |
| 无 lint 压制 | 成立 | 全仓库无 `#[allow(...)]` |
| 无调试残留 | 成立 | 无 `dbg!` / `TODO` / `FIXME` / `console.*` / `debugger` |
| 无未跟踪文件 | 成立 | `git ls-files --others --exclude-standard` 为空 |
| IPC 面收敛 | 成立 | 14 条全部有业务含义，窗口开关类调试命令已删除 |
| 权限最小化 | 成立 | 仅 `core:default`；前端只用 `invoke` 与 `event.listen` |
| CSP 生效 | 成立 | `default-src 'self'` 等，上线后端到端仍全绿 |

## 本阶段（R01–R10）完成项

| 轮 | 内容 | 关键证据 |
|---|---|---|
| R01 | 空态两步操作指引 | 探针断言标题/两步/键帽，端到端 41→44 |
| R02 | 已在早期实现（`show_on_launch` 不上膛失焦收走） | `hotkey.rs` 的 `AUTO_DISMISS_ARMED` |
| R04 | 摆放几何抽纯函数 + 7 条单测 | 含负坐标副屏与"窗口大于工作区"两个端到端覆盖不到的用例 |
| R05 | 按实测对比度修配色 | `--ink-soft` 3.59→4.58:1；窗口发丝边白底 1.07→约 2.1:1 |
| R06 | 格式闭包 5 组测试 | 实体转义、伪 marker、零宽/ZWJ、2000 字长文 |
| R07 | 外部占用不再永久降级 | `IoKind::Busy` + 短重试；`retry_rides_out_a_short_lock` 证明重试有用 |
| R09 | 损坏文件角标与定位 | 实测"2 个损坏文件"，干扰文件未被计入 |
| R10 | 本报告 | — |

## 明确未验证的部分（不假装已覆盖）

1. **混合 DPI（100% / 125% / 200%）**：本机所有显示器同缩放，无法验证。
2. **负坐标副屏的端到端**：当前单屏，脚本跳过；几何已由单元测试覆盖，但真实窗口行为未验。
3. **视觉部分**：透明窗口是否真透出桌面、边缘观感、IME 候选框跟随——需要人眼。
4. **4 项性能测试**默认不跑（需 `--ignored`），门禁不覆盖性能回归。

## 未做与原因

| 轮 | 不做原因 |
|---|---|
| R03 托盘通知 | 需要 `notification` 插件 + 新权限，须单独评审 |
| R08 复制诊断 | 需要 `clipboard` 插件 + 新权限，须单独评审 |
| R13 / R22 / R25 | 与「不重新引入物理归档」「append+patch 变更词汇」冲突，已确认不做 |
| R48 `panic = "abort"` | 打死 `JoinError` 契约，容灾能力归零，已确认不做 |
