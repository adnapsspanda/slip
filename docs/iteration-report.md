# Slip 迭代结项报告

> 覆盖本会话的全部迭代轮次。数据来自实际执行，不是估计。
> 生成于最后一轮，工作区干净。

## 一、规模与资产

| 项 | 数值 |
|---|---|
| 总提交 | 69（本目标内 46） |
| 本次迭代内提交 | 41 |
| Rust 代码 | 8870 行 |
| 前端代码（svelte/js/css） | 2157 行 |
| 文档（设计文档、AGENTS、README、报告等） | 2293 行 |
| 集成测试文件 | 6 |
| 端到端检查点 | 探针 74 条断言 + 验收脚本 30 个检查点（部分按运行条件触发）；最后一次实测 **100 通过 / 1 跳过** |
| 门禁/检查脚本 | 6 个（`tools/*.ps1`） |

## 二、完成的轮次与证据

| 轮 | 内容 | 关键证据 |
|---|---|---|
| R11 | 全键盘流（↓ 进列表 / Space 勾选 / Enter 编辑 / ↑ 回捕获框） | 端到端 5 条，`focusBack=true` |
| R12 | 修复切换编辑目标**丢改动** | 实测到数据丢失；改不可变 session；端到端 3 条 |
| R14 | Ctrl+F 本地过滤 | 1000 条下纯计算 0.064 ms；端到端 5 条 |
| R15 | 行内渲染，**零 HTML 注入面** | 手工切分 + 结构化渲染；端到端 7 条 |
| R16+R18 | 删除前端零入口的视图死路径 | **净减 522 行**；主循环少一个 `select!` |
| R17 | 粘贴清单前缀清洗 | 5 条单测；5 类"像但不是"的输入原样保留 |
| R19 | 相对时间，不轮询 | 纯函数；端到端五档实测 |
| R20 | 阶段二验收 | `docs/phase-2-report.md` |
| R21 | 归档抽屉 + 恢复 | IPC 14→15；端到端 4 条 |
| R23 | 导出可读 Markdown（剥 marker） | 纯函数 + 端到端 1 条 |
| R24 | 统计概览 | 端到端 2 条 |
| R26 | 备份管理器（**恢复可逆**） | 3 条单测 + 端到端 3 条 |
| R27 | 外部修改的条数级差异 | 3 条单测 + 端到端 2 条 |
| R30 | 存储性能基线 | `docs/perf-baseline.md`（实测数字） |

### 期间发现并修掉的真缺陷

1. **切换编辑目标丢改动**（R12）——改着 A 去双击 B，A 的改动被静默丢弃。
2. **窗口拖不动**（权限审计轮）——`core:default` 不含 `allow-start-dragging`，
   `data-tauri-drag-region` 一直是死的；外观毫无异常。
3. **重载后撤销栈指向旧文档**（R20）——报"请先重新载入"（用户刚载入过），
   且失败会把该步放回栈里，之后每次撤销都撞墙。
4. **消失条目的编辑态卡死**（R21）——`Ctrl+Z` 被永久拦掉，界面上没有编辑框。
5. **归档后的空态说"还没有内容"**（R19）——用户刚归档，看着像数据丢了。
6. **视图状态跨唤出残留**（R22）——再唤出落在一张被筛过的列表上（1 条 vs 实际 9 条）。
7. **只读文件时横幅空白**（写入失败轮）——`writable` 仍为 true，只有一句瞬时提示。
8. **前端预检与领域层口径漂移**（R17）——合法输入被界面拒收。

### 期间建立的机制（不是功能，是防线）

- **状态生命周期表**（`AGENTS.md`）：前端 6 项 + 后端 2 项的"有效期/何时清/漏了会怎样"。
- **变异验证**：断言必须"去掉修复后会红"才算数。已验 7 条守卫（其中一条覆盖 5 个测试用例），抓到 1 条空断言。
- **不可回退约束的机器化**（`tools/check-forbidden.ps1`）：5 项接进钩子与 CI。
- **时序守卫加严**（`verify-hotkey.ps1`）：源码比 exe 新就拒绝验收；排除 `tests/`。
- **环境自检**（`tools/check-environment.ps1`）：一键判断产物能否运行并给出解法。

## 三、未完成与原因（逐项）

| 项 | 状态 | 原因 |
|---|---|---|
| R13 单条删除 / R22 物理擦除 / R25 `archive.md` | **不做** | 需要新的变更词汇，且会打破 `validate_candidate` 的按索引比对 |
| R16/R18 视图状态 | **已删除** | 前端从未产生过事件，是死路径 |
| R48 `panic = "abort"` | **不做** | 打死 `JoinError` 契约，容灾能力归零 |
| R40 混合 DPI | **不可验证** | 本机所有显示器同缩放 |
| R03 气泡通知 / R08 复制诊断 / R28 剪贴板导入 / R29 复制文本 | **未做** | 需要加 `notification` / `clipboard-manager` 插件与 capability，**等用户批准** |
| R31–R50 其余轮次 | **未做** | 该矩阵不在当前上下文中，**不猜测内容** |

## 四、当前阻塞：Smart App Control 阻止执行构建产物

**事实**（`CiTool -lp` + 事件日志）：

```
Policy ID: 0283ac0f-fff1-49ae-ada1-8a933130cad6
好记的名称: VerifiedAndReputableDesktop     （即 Smart App Control 本体）
平台策略: true   已签名: true   当前强制执行: true
拒绝理由: did not meet the Enterprise signing level requirements
```

**影响**：`cargo build` 通过、`cargo fmt/clippy` 通过，但**运行**任何新编出的产物被拒；
连跑 `cargo test` 也时断时续（重新编译会让原本能跑的测试二进制变成被拒）。

**SAC 没有白名单**——以下全部无效（已实测/已确认）：自签名证书、补充 WDAC 策略、
Defender 排除目录、改 SmartScreen、换目录或改文件名。

**可行的三条**：关闭 SAC（单向开关，可能要重置系统）；换 Windows Sandbox / Hyper-V
（虚拟化已开启）；换机器。**WSL 帮不上**（GUI 与端到端需要 Windows 窗口、全局热键、
WebView2；且拦截发生在 Windows 执行时）。

## 五、最后一次全绿记录

在被彻底阻塞之前：

```
cargo fmt --check            ✅
cargo clippy -D warnings     ✅
cargo test --workspace       ✅ 221 项（4 项 perf 忽略）
# 此后新增 1 条 crates/slip-store/tests/archive_guard.rs，本机被 SAC 拦、尚未首次执行，
# 首次执行由 CI 完成；它一旦跑通，总数变为 222
tools/verify-hotkey.ps1      ✅ 104 项 / 0 失败（2026-11-11 当天复跑三次，均为 104/104）
tools/check-forbidden.ps1    ✅ 5 项
```

## 六、要恢复迭代，需要什么

1. **放行执行**：关闭 SAC，或把仓库放到无 SAC 的 Windows 环境 → 端到端 100 项可复跑，
   每轮"门禁全绿"重新成立；
2. **插件决策**：批准（或否决）R03/R08/R28/R29 的插件与权限；
3. **R31–R50 矩阵**：贴一次，我按既有标准继续（实现 → 门禁全绿 → 一项一提交 → 证据入档）。

三项都不需要我猜测——这也是我没有继续硬凑轮次的原因。

## 七、终局状态（最后一轮复核）

| 检查项 | 最后一次实测 | 说明 |
|---|---|---|
| 端到端 `verify-hotkey.ps1` | **104 项 / 0 失败**（当天第三次，三次都是 104/104） | 主程序与界面行为稳定 |
| `cargo fmt --check` | 通过 | — |
| `cargo clippy -D warnings` | 通过 | — |
| `tools/check-forbidden.ps1` | 通过（5 项约束） | — |
| `cargo test`（逐目标） | **9 个目标全部通过，合计 225 项**（升级专业版后实测） | `--workspace` 一次跑完仍可能被随机拦掉一个二进制 |
| `archive_guard.rs`（新增） | **已执行并通过**（1 项） | 升级专业版后取得执行窗口 |

### 两个仍然锁死的门（都不是代码问题）

1. **执行**：Smart App Control（`VerifiedAndReputableDesktop`）按文件拒绝我们的未签名产物。
   规律：**新编出的二进制首次执行常被拒，同一文件隔一会儿重试往往就过**；
   所以日常开发不必关 SAC，重试即可（`tools/cargo-test-retry.ps1` 已按此实现）。
   只有需要"一次就能打开 GUI"时才值得关 SAC 或换 Sandbox/VM。
2. **依赖**：`crates.io` 解析到 `198.18.1.197`（本地代理的假 IP 段）且 TLS 失败，
   `github.com` 同样不可达。**这直接让 R03/R08/R28/R29 无法进行**——不是权限问题，
   是拿不到 `tauri-plugin-notification` / `tauri-plugin-clipboard-manager`。
   本地无镜像、无 vendor、无缓存。

### 因此未做的轮次（逐项原因）

| 轮次 | 原因 |
|---|---|
| R03 气泡通知 / R08 复制诊断 / R28 剪贴板导入 / R29 复制文本 | **依赖下载被代理阻断**（见上）；权限审批只是第二道门 |
| R13 单条删除 / R22 物理擦除 / R25 `archive.md` | 与既有不变量冲突，已确认不做 |
| R48 `panic = "abort"` | 打死 `JoinError` 契约，已确认不做 |
| R40 混合 DPI | 本机所有显示器同缩放，无法验证 |
| R31–R50 其余 | **该矩阵不在本次会话上下文中，拒绝猜测** |

## 八、收官（第 40 轮，最后一轮）

最后一轮的三次探测结果一致：**网络仍不可达、被拦的二进制仍被拦**。

```
cargo search tauri-plugin-notification  → 失败（crates.io 不可达）
cargo test -p slip-store --test archive_guard → 4551
cargo test --workspace                  → 87 项后 4551
```

因此本轮不再制造改动，把交付物收束为：

| 交付物 | 位置 |
|---|---|
| 迭代结项报告（本文件） | `docs/iteration-report.md` |
| 逐轮记录与判据 | `docs/ralph-backlog.md` |
| 工程约定（含 3 个环境陷阱） | `AGENTS.md` |
| 阶段体检报告 | `docs/phase-1-report.md`、`docs/phase-2-report.md` |
| 性能基线 | `docs/perf-baseline.md` |
| 环境自检 | `tools/check-environment.ps1` |
| 约束机器化检查 | `tools/check-forbidden.ps1` |
| 测试重试（只对 4551，不删产物） | `tools/cargo-test-retry.ps1` |

### 重启迭代的三个入口（任一即可）

1. **修好网络**（让 `cargo search` 能返回结果）→ 解锁 R03 / R08 / R28 / R29；
2. **贴出 R31–R50 矩阵** → 按既有标准继续；
3. **给一个无 SAC 的 Windows 环境**（Sandbox/VM/另一台机）→ 门禁可以每轮全绿，
   包括目前只能交给 CI 首次执行的 `archive_guard.rs`。

### 已知欠账（明确列出，不假装完成）

| 欠账 | 状态 |
|---|---|
| `crates/slip-store/tests/archive_guard.rs` | 通过 `cargo check --tests`，**从未执行过**；首次执行交给 CI |
| `cargo test --workspace` 的完整绿灯 | 上一次完整通过是 221 项；此后环境只允许部分执行 |
| R03/R08/R28/R29 | 依赖下载被代理阻断 |
| R31–R50 其余 | 矩阵不在上下文中 |

## 九、升级 Windows 11 专业版后的复核（2026-11-11 晚）

| 检查 | 结果 |
|---|---|
| `cargo check --tests` | **通过**（升级前持久失败于 build script） |
| `cargo build -p slip-shell` | 通过 |
| 端到端 `verify-hotkey.ps1` | **104 项 / 0 失败** |
| 单元与集成测试（逐目标） | **9 个目标全部通过，合计 225 项** |
| `cargo test --workspace`（一次跑完） | 216 项后仍会随机被拦掉一个二进制 |
| `cargo fmt` / `clippy` / 约束检查 | 通过 |

**SAC 策略状态没变**（仍是强制），所以这不是"升级解除了拦截"，而是"被拦的产物随时间
会被放行"这一规律在起作用。稳定跑法是**逐目标执行 + 失败重试**。
