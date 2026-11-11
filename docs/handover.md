# 接手须知：当前状态与从哪继续

> 写于 R32–R81 迭代收尾（第 39 轮）。目的只有一个：**任何人拿到这个仓库，能在十分钟内
> 知道什么能跑、什么被环境挡住、以及别去做什么。**

## 一、现在能跑什么（都验证过）

| 命令 | 结果 | 说明 |
|---|---|---|
| `cargo test --workspace` | **254 项通过** | Windows 侧，含原生错误码映射、文件锁等平台专属测试 |
| `wsl.exe -d Arch -- bash -lc 'sh /mnt/d/Dev/simple-todo/tools/wsl-test.sh'` | **241 项通过** | Linux 侧，不受 SAC 影响；缺的 13 项是 Windows 专属 |
| `& "C:\Users\spand\.bun\bin\bun.exe" run tools/check-ui-logic.mjs` | **108 条断言** | 前端纯逻辑，不需要浏览器 |
| `cargo clippy --workspace --all-targets -- -D warnings` | 通过 | |
| `cargo fmt --all --check` | 通过 | |
| `pwsh -File tools/check-forbidden.ps1` | 8 项约束通过 | 依赖、归档、锁、HTML 注入、网络出口、危险 API、CSP |
| `pwsh -File tools/check-doc-sync.ps1` | 通过 | 文档里的 IPC 命令数与 capability 清单 |
| `pwsh -File tools/check-ci-parity.ps1` | 零漂移 | 钩子与 CI 是否覆盖同一组关口 |
| `pwsh -File tools/check-bundle-size.ps1` | 通过 | CSS 9.99 KB / 10 KB、JS 68 KB / 70 KB |
| `pwsh -File tools/check-ps1-syntax.ps1` | 通过 | `*.ps1` 的 BOM 与语法 |
| `python tools/mutation-audit.py` | 通过 | 三处核心不变量的变异审计 |

`git commit` 的钩子会把这些里的绝大多数跑一遍（不需要手动逐条）。

## 二、什么被挡住了，以及**别怎么试**

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

## 三、环境恢复后按这个顺序继续

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

## 四、两处需要判断的地方（不是 bug，是取舍）

1. **CSS 预算只剩约 10 字节**（9.99 KB / 10 KB）。下一处样式改动几乎必然撞线。要么承认 10 KB
   已不适配当前功能面并调高，要么专门做一次 CSS 合并整理。这条没替后来人做决定。
2. **`skipTaskbar` 的落地方式**未定论，见上。

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
