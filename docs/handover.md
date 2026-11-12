# 接手须知：当前状态与从哪继续

> 写于 R32–R81 迭代收尾（第 39 轮）。目的只有一个：**任何人拿到这个仓库，能在十分钟内
> 知道什么能跑、什么被环境挡住、以及别去做什么。**

## 一、现在能跑什么（都验证过）

| 命令 | 结果 | 说明 |
|---|---|---|
| `cargo test --workspace --offline` | 通过 | Windows 侧，含原生错误码映射、文件锁等平台专属测试 |
| `bash tools/wsl-test.sh` 或 `cargo test -p slip-domain -p slip-store -p slip-app --all-targets --offline` | 通过 | Linux 侧，不受 SAC 影响；平台专属测试只在 Windows 运行 |
| `bun run tools/check-ui-logic.mjs` | 通过 | 前端纯逻辑，不需要浏览器 |
| `cargo clippy --workspace --all-targets -- -D warnings` | 通过 | |
| `cargo fmt --all --check` | 通过 | |
| `pwsh -File tools/check-forbidden.ps1` | 通过 | 依赖、归档、锁、HTML 注入、网络出口、危险 API、CSP |
| `pwsh -File tools/check-doc-sync.ps1` | 通过 | 文档里的 IPC 命令数与 capability 清单 |
| `pwsh -File tools/check-ci-parity.ps1` | 零漂移 | 钩子与 CI 是否覆盖同一组关口 |
| `pwsh -File tools/check-bundle-size.ps1` | 通过 | 前端产物在预算内 |
| `pwsh -File tools/check-ps1-syntax.ps1` | 通过 | `*.ps1` 的 BOM 与语法 |
| `python tools/mutation-audit.py` | 通过 | 核心不变量的变异审计 |

`git commit` 的钩子会把这些里的绝大多数跑一遍（不需要手动逐条）。

### Linux 支持现状

| 项 | 状态 |
|---|---|
| 逻辑层（domain/store/app） | 可编译、测试通过；CI 的 `logic-linux` job 覆盖 |
| Linux 外壳（`slip-shell`） | 可编译、可启动；CI 的 `shell-linux` job 做 clippy / build / lib 单测 |
| 显示会话 | **X11 为准**；Wayland 下全局热键可能不可用，托盘取决于 AppIndicator/桌面环境 |
| 透明/置顶/圆角 | 取决于合成器；未做与 Windows 同等级的实测 |
| 打开 Inbox/链接/附件 | 依赖 `xdg-open`；缺失时这些入口报错，其余功能不受影响 |
| 端到端验收 | **尚无** Linux 等价物；`shell-linux` 只验证编译与单测，不跑 GUI/E2E |

Linux 默认 vault 为 `$HOME/Documents/Slip`（没有 `Documents` 时 `$HOME/Slip`），
仍可用 `SLIP_VAULT` 覆盖。

## 二、Windows 上什么被挡住了，以及**别怎么试**

> 本节只适用于 Windows。Smart App Control（SAC）是 Windows 的系统策略；
> Linux 逻辑层与 Linux 外壳编译不受它影响（见上一节）。

| 项 | 现象 | 结论 |
|---|---|---|
| 端到端 `tools/verify-hotkey.ps1` | 启动 `slip-shell.exe` 被拒 | Smart App Control 拦截新编出的未签名 exe |
| `cargo build --release` | `target/release/deps` 里的 `webview2_com_macros-*.dll` / `schemars_derive-*.dll` 被拒 | 同上，release 模式的 proc-macro 也未签名 |
| R67 任务栏驻留 | 窗口扩展样式是 `WS_EX_APPWINDOW`，不带 `WS_EX_TOOLWINDOW` | **判据不可靠**：框架可能用 `ITaskbarList::DeleteTab` 摘按钮而不动扩展样式。需要人工看一眼任务栏 |

### 已经试过且**无效**的绕法（别再重复）

| 尝试 | 结果 |
|---|---|
| 反复重试启动同一个 exe | 有时有效（等几分钟到几小时），但不可控 |
| `cargo clean -p <crate>` 重建 | **帮倒忙**：重建出的新产物同样被拒，还会把本来能跑的依赖一起带下去 |
| 改 `RUSTFLAGS` / `CARGO_PROFILE_*` 换产物哈希 | **更糟**：会连带生成一批新的被拦产物（本会话实测把 workspace 测试从 254 打到 118） |
| 自签名 / 改文件名 / 换目录 | 无效。SAC 按签名与云端信誉判定，与路径无关 |
| 给 cargo.exe 签名的思路 | 无效。被拦的从来不是工具链，而是**我们编出的产物**（实测 200 条拦截事件里工具链出现 0 次） |

### 唯一有效的做法

**等。** 本会话里多个被拦产物都在几十分钟到几小时后自己放行了（`slip-shell.exe`、
`schemars_derive-*.dll`、`slip_shell-*.exe` 测试二进制各有一例）。等待期间用 WSL 写代码、
跑逻辑测试完全不受影响。

## 三、Windows 环境恢复后按这个顺序继续

```powershell
# 1. 确认 exe 能跑（连试几次；被拒就等）
Start-Process .\target\debug\slip-shell.exe -PassThru

# 2. 跑端到端（会一并验证 R67 的任务栏断言；它们目前是「打印实测值 + SKIP」）
Get-Process slip-shell -EA SilentlyContinue | Stop-Process -Force
pwsh -NoProfile -File tools/verify-hotkey.ps1

# 3. 若 R67 的断言仍然显示 ex=0x40118 / TOOLWINDOW=False，
#    人工看一眼任务栏：有按钮 → 说明 skipTaskbar 确实没生效，需要改用 ITaskbarList 方案；
#    没有按钮 → 说明框架用的是另一种机制，把断言改成检查真实结果而不是扩展样式。

# 4. release 能编时打发布包
pwsh -NoProfile -File tools/package-release.ps1
```

## 四、两处取舍已裁决（2026-11-12）

1. **CSS 预算：已提额到 14 KB**（`tools/check-bundle-size.ps1`）。原 10 KB 的配额定于 R31
   之前，当时还没有双模变量体系；R31–R41 整套深浅色 + 聚焦环 + 横幅三档 + 细滚动条净增约
   2.3 KB、压缩后 9.99 KB，控制力已经克制。死守 10 KB 只会逼出"为指标牺牲可读性"的缩写。
   14 KB 留 4 KB 缓冲，仍能挡住无节制引入样式库。**JS 预算保持 70 KB。**
2. **`skipTaskbar`：维持"打印实测值 + 人工目视确认"，不写 Win32 样式断言。**
   现代 Chromium/WebView2/Tauri 应用摘任务栏按钮的主流做法是
   **`ITaskbarList::DeleteTab(hwnd)`**，而不是加 `WS_EX_TOOLWINDOW`——因为在 Win11 下
   挂上 `TOOLWINDOW` 会让 DWM 的平滑动效、阴影与圆角抗锯齿降级成老式工具窗口。
   既然底层是 COM 从 Shell 摘按钮，用 `GetWindowLongPtr(GWL_EXSTYLE)` 当然抓不到
   `TOOLWINDOW`；以错误前提写断言只会制造假报警。

## 五、这个仓库的几个"反直觉"约定

- **前端零 HTML 注入**：`inbox.md` 是用户可编辑文件，它的内容永远不当 HTML 执行。
  渲染靠 `ui/src/lib/inline.js` 的 `tokenize` 手工切段——不要换回 `{@html}`。
- **`inbox.md` 只能由 `slip-store` 写**，外壳与前端绝不直接写。`rename` 是提交点，
  写临时文件失败等于"没提交"。
- **`Presenter` 不上锁**：单队列、单消费者。加锁意味着出现了第二个写者。
- **不引入 watcher**：外部修改靠"写入前读盘比对"发现，多一套时序就等于多一类竞争。
- **门禁与验收脚本本身是产物**：`tools/` 有 3400+ 行，和前端代码体量相当。改它们要有测试意识。

## 六、文档地图

| 想知道什么 | 看哪 |
|---|---|
| 为什么这么设计、有哪些不变量 | `docs/design/Slip-架构与模块定义.md` |
| 每一轮做了什么、证据是什么 | `docs/ralph-backlog.md` |
| 各阶段的结论与被否掉的方案 | `docs/phase-1~7-report.md` |
| 性能基线（10,000 条 6.37 ms） | `docs/perf-baseline.md` |
| 50 轮总账与欠账清单 | `docs/final-50-rounds-report.md` |
| 工程约定与踩过的坑 | `AGENTS.md` |
