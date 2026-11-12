# Repository Guidelines

Slip 是召唤式待办捕获工具：热键弹出纸片，输入即落进本地 `inbox.md`。
Windows 是主平台（Tauri + WebView2）；Linux 外壳已支持，**以 X11 为准**。
三个逻辑 crate 跨平台，Linux 上可直接跑测试。

**接手先读：**

- `docs/design/Slip-架构与模块定义.md` —— **唯一真源**。实现与文档冲突时先改文档。
- `docs/handover.md` —— 当前能跑什么、什么被环境挡住、哪些绕法已证明无效（别再试）。
- `docs/ralph-backlog.md` —— 每轮改动的证据与已知欠账。

## 结构边界

```
crates/slip-domain/   纯领域：文本规范化、拆分、状态转换、ID/时间格式。零 I/O。
crates/slip-store/    唯一磁盘写入者：Markdown 解析/渲染/局部替换 + 事务层 + 偏好。
crates/slip-app/      命令队列、消费者、presenter、UiSnapshot。不含 tauri，可纯单测。
src-tauri/            Tauri 外壳：窗口、热键、托盘、IPC。薄，不放业务状态。
ui/                   Svelte 5 + Vite；`bun run build` 产物进 `ui/dist` 后被编译期嵌入。
tools/                门禁、验收、诊断脚本。改它们要有测试意识。
```

依赖方向**只能**是 `src-tauri → slip-app → slip-store → slip-domain`，反向一律拒绝。
`inbox.md` 的读写只允许经 `slip-store`；外壳只能导出附件、只读枚举 `inbox.md.corrupt-*`。
前端不碰文件系统，也**不得**发起网络请求或注入 HTML。

## 命令

本机 `cargo` 可能不在 PATH：先补 `~/.cargo/bin`（Windows 为 `%USERPROFILE%\.cargo\bin`）。

| 目的 | 命令 |
|---|---|
| 逻辑层测试（Linux/WSL） | `cargo test -p slip-domain -p slip-store -p slip-app --all-targets --offline` |
| 全量测试（Windows） | `cargo test --workspace --offline` |
| 单个测试 | `cargo test -p <crate> --lib <过滤词>` |
| 性能测量 | `cargo test --release -p slip-store --test perf --offline -- --ignored --nocapture` |
| 静态检查 | `cargo clippy --workspace --all-targets --offline -- -D warnings` |
| 格式 | `cargo fmt --all` / `cargo fmt --all --check` |
| 前端 | `cd ui && bun install && bun run build`（`bun run format:check` 查格式） |
| 跑外壳（Windows，主平台） | `cargo run -p slip-shell` |
| 跑外壳（Linux，X11 为准） | 先装下方系统依赖并构建前端，再 `cargo run -p slip-shell` |
| Linux 外壳构建/单测 | `cargo build -p slip-shell --offline` / `cargo test -p slip-shell --lib --offline` |

Linux 系统依赖（Tauri v2）：

```bash
# Arch
sudo pacman -S --needed base-devel webkit2gtk-4.1 gtk3 libappindicator librsvg patchelf openssl

# Debian / Ubuntu
sudo apt install libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev \
  librsvg2-dev patchelf libssl-dev
```

- Linux 外壳需要图形会话；只验证编译/单测时用 `cargo test -p slip-shell --lib`，不需要桌面。
- 打开 Inbox、打开链接、定位附件依赖 `xdg-open`；缺失时这些入口会报错，其余功能不受影响。
- 首版以 **X11** 为准：Wayland 下全局热键可能不可用；托盘行为受 AppIndicator / 桌面环境影响；
  透明窗口、置顶与圆角取决于合成器。Linux 暂无与 `verify-hotkey.ps1` 等价的端到端验收。

- 依赖已缓存，**优先加 `--offline`**；新增依赖时去掉一次。
- **干净克隆的硬顺序**：`ui/dist` 不入库，而 `tauri::generate_context!` 在**编译期**读它。
  任何包含 `src-tauri` 的 cargo 命令之前，必须先 `cd ui; bun install; bun run build`，
  否则只会得到 `proc macro panicked`（`src-tauri/build.rs` 会提前打印正确命令）。
  改完界面也必须重建前端再跑外壳。
- 领域/存储/调度测试必须 1 秒内跑完；变慢通常意味着有人偷偷加了 I/O。
- 性能测试全部 `#[ignore]`；引用数字必须连构建配置一起引用（debug 与 release 差 3.5 倍）。
- 日志走 `tracing` 到 stderr，`SLIP_LOG=error|warn|info|debug|trace`（默认 `info`）。
  `SLIP_VAULT` 可指定 vault 目录；默认 Windows 为 `%USERPROFILE%\Documents\Slip`，
  Linux 为 `$HOME/Documents/Slip`（没有 `Documents` 时 `$HOME/Slip`）。用于测试与多实例。
- 提交钩子每克隆装一次：`git config core.hooksPath .githooks`。
  `--no-verify` 只能跳过本地钩子，CI 仍会拦。

## 质量门禁

`git commit` 会跑：`*.ps1` 的 BOM → PowerShell 语法 → prettier → 前端纯逻辑 →
产物体积 → `fmt --check` → `clippy -D warnings` → `cargo test --workspace`。
CI（`.github/workflows/ci.yml`）还跑前端构建、release 构建、Linux 逻辑层 job，
以及 Linux 外壳 job（`shell-linux`：编译 + lib 单测，不跑 GUI/E2E）。
`tools/check-ci-parity.ps1` 检查钩子与 CI 是否零漂移；改门禁时两边一起改。

- `tools/check-forbidden.ps1` 是"不可回退约束"的文本守卫（依赖、归档、presenter 上锁、
  `{@html}`、网络出口、`eval`、CSP）。`-SelfTest` 用正负样本验证守卫本身没写死。
- 工具链由 `rust-toolchain.toml` 钉死（1.99.1 + clippy + rustfmt）。**不要随手升级**：
  新 rustc 的新 lint 会让与本次改动无关的代码突然变红。

## 产品形态：召唤式（改动窗口/热键前必看）

窗口默认隐藏（`tauri.conf.json` 的 `visible: false`），托盘是常驻入口。四条不能破坏：

1. **手动双击必须看得见窗口。** 自启插件带 `--autostart` 才静默驻留；否则 setup 调
   `hotkey::show_on_launch`。改掉这条 = 用户双击 exe 后"什么也没发生"。
2. **唤出贴光标**：右下 16 逻辑像素，放不下翻左上，最后夹进 `Monitor::work_area()`；
   任何信息拿不到都退化为居中。
3. **失焦自动收走，但只对"召唤"上膛**：热键/托盘走 `summon`（上膛）；启动展示走
   `show_on_launch`（不上膛，别的应用抢前台不该让窗口消失）。宽限 400ms，且
   **从未被聚焦过的窗口不因失焦收走**。
4. 热键存在 `<vault>/ui-state.json` 的 `hotkey` 字段。`Prefs` 是**整体写**的：
   运行期换键必须读-改-写（`system::persist_hotkey`），不要另造一个只含 `hotkey`
   的 `Prefs` 覆盖整份文件。视图快照不再携带偏好——`PaperViewSet` 已于
   2026-11-12 删除（生产没有视图写入路径，留着只会诱导后来者再踩一次"少搬字段"）。

## 状态生命周期（这类 bug 会伪装成"功能坏了"）

新增一个有生命周期的状态，必须同时回答：**何时失效、谁负责清、漏了会怎样。**
症状一律是静默错误或毫无反应，不报错、不崩溃、测试也常照样通过。

| 状态 | 必须在何时清 | 漏掉后的症状 |
|---|---|---|
| `filter` / `filtering`、`showArchived` | `summon()` | 再唤出停在上一轮筛过的列表/归档抽屉 |
| `showBackups` / `backups` / `pendingRestore` | `summon()`、面板关闭、恢复成功 | 残留的"确认？"恢复的是旧选择 |
| `notice`（撤销提示） | `summon()`、`reload`、`isolate`、`restore` | 按钮还在但后端已无对应撤销 |
| `armed`（二次确认武装态） | `summon()`、执行/超时、窗口收起 | 再唤出时一次单击执行不可逆操作（彻底删除） |
| `preview`（损坏文件只读预览） | `summon()`、整库替换（reload/restore/isolate） | 只读弹窗跨会话残留，`Ctrl+Z`/`Ctrl+F` 被永久拦 |
| `editingId` / `session` / `draft` | 该条目从 `slips` 消失时 | `editing` 恒真 → `Ctrl+Z` 被永久拦掉 |
| `inFlight` / `draftVersion` | 提交结束 | 迟到的 ACK 把新草稿误判成自己的 |
| `pendingDone` / `pendingToggleCounts`（未落盘的勾选意图） | 该 id 无排队请求且失败/快照已追上、`summon()` | 勾选要按两下：下一次点击从"没发生过的状态"反转；第三次点击落到错误终态 |
| 撤销栈 | 任何"整体替换内容"路径：`Reload`、`RestoreBackup`、`IsolateAndReset` | 逆操作指向不存在的条目，之后每次撤销都撞同一堵墙 |
| `needs_reload` | 成功写入或成功 `reload` | 永远写不进去 |

判据：整体替换内容的路径必须清空一切基于旧内容的派生状态；临时视图状态不跨唤出；
有条件渲染的编辑框其状态必须随数据消失而失效。加新路径时搜一遍 `Command::` 枚举逐个核对。

## 不可回退的约束

改这些之前先读设计文档对应小节：

- 未修改行**逐字节保留**；候选文档只能由原始行数组局部替换构造，禁止全文重序列化。
- 提交点 = `rename` 成功；之后的失败只能记 `DurabilityUnconfirmed`，不得报失败。
- 单队列单消费者：`presenter` 由消费者独占，不引入锁或版本号过滤。
- `ui_rev` 在**每一次**生成并发布的快照上递增（含视图事件）；`GetSnapshot` 不递增。
- 禁止引入：文件 watcher、内容去重、回调/扩展点、事件总线、SQLite 索引、
  物理归档目录、跨字段或时钟合法性校验。

这些约束由测试与 `check-forbidden.ps1` 分工守着（"禁止全文重序列化"只能靠往返测试）。
新增约束时，同时补上守护方式或在 `docs/ralph-backlog.md` 写明为何无法自动覆盖。

## 测试与变异验证

"测试通过"只说明当前实现下没红；**去掉修复后测试会红**才是证据。

1. 先确认基线能跑，再动手改（环境可能几分钟内从能跑变成任何新产物都被拒）。
2. 一次只改一处，把修复临时改坏，跑那条测试确认**变红**，然后立刻还原。
3. 还原后核验 `git status` 干净，并肉眼复看被改的行（替换脚本可能没匹配上却打印 restored）。
4. 修 bug 必须同时补一条能复现的回归测试。

领域层与调度层用**纯内存**测试，禁止碰真磁盘；事务层与格式闭包必须用**真实临时目录 +
真实文件**（`tests/acceptance.rs`），因为它们验证的正是"磁盘上到底发生了什么"。
测试名说清行为，例如 `noop_patch_does_not_rewrite_the_file`。

## 环境陷阱（Windows 开发机）

- **`os error 4551`（Smart App Control）**：新编出的未签名 exe 会被系统策略拒绝执行。
  先跑 `pwsh -File tools/check-environment.ps1`。标准应对是**重试同一个二进制**：
  `pwsh -File tools/cargo-test-retry.ps1`。**不要 `cargo clean` 重编**——每次重编都是
  新文件，又要重新经历"首次执行被拒"。同一份产物过几分钟到几小时可能自己放行。
  被拦的永远是产物，不是 cargo/rustc；重装工具链、签名、加 WDAC、Defender 排除都无效。
- **`cargo test --workspace` 可能随机被拦掉一个二进制**：失败时先怀疑 SAC，别怀疑代码；
  逐目标执行 + 重试最稳。
- **可以在 Linux/WSL 跑三个逻辑 crate 绕开 SAC**：`tools/wsl-test.sh`（CI 的
  `逻辑层（Linux）` job 做同样的事）。平台相关测试必须 `#[cfg(windows)]`——原生错误码
  （32/33 共享冲突、5 拒绝访问）在 Linux 上含义完全不同。
- **crates.io 可能被本地代理劫持**（解析到 `198.18.x.x`，报 schannel 握手失败）。
  加依赖前先 `cargo search <crate> --limit 1`；失败是网络/代理问题，不是权限问题。
- **端到端验收 `pwsh -File tools/verify-hotkey.ps1` 只能在交互桌面跑**。沙盒终端里
  启动的进程窗口不可见、`cursor_position()` 读不到光标（指纹：多次启动坐标都固定在屏幕
  正中、`focused: false`）。这不是程序缺陷。若窗口存在但 `IsWindowVisible` 为假，
  用 `explorer.exe .\target\debug\slip-shell.exe` 启动对照；并与 `git stash` 后的上一版
  对比，先排除环境假红。
- **改 `tools/*.ps1` 必须带 UTF-8 BOM**，否则中文 Windows 按 GBK 解码直接语法错。
  `edit`/`write` 会丢 BOM，改完补回。PowerShell 把 `$null` 传给 `[string]` 会变空串，
  要真 NULL 用 `[NullString]::Value`。PowerShell 还是 DPI 不感知进程，屏幕尺寸是虚拟值。
- **截图不能证明透明**：DirectComposition 透明区在 `BitBlt` 上会渲染成近黑，只能人眼判定。
- 写验收断言不要依赖未声明前提：窗口按进程 ID 找（`Tauri Window` 类名会被其他应用共用），
  坐标由 `PrimaryScreen.Bounds` 推导，不要把显示器数量/光标位置写死。

### Linux 开发环境

- 外壳编译失败先看系统依赖：`webkit2gtk-4.1`（Debian/Ubuntu 为 `libwebkit2gtk-4.1-dev`）、
  GTK3、AppIndicator、`librsvg`、`patchelf`、OpenSSL。命令见上方「Linux 系统依赖」。
  这些只影响 `slip-shell`；三个逻辑 crate 不依赖它们。
- 运行必须处于图形会话。纯 SSH/无 `DISPLAY` 时只能编译与跑 lib 单测，
  启动窗口会失败——这不是代码回归。
- **X11 是首版基准**。Wayland 下全局热键可能注册不上；托盘是否出现取决于
  AppIndicator 与桌面环境；透明/置顶/圆角由合成器决定。遇到"热键没反应/托盘不见了"
  先确认会话类型（`echo $XDG_SESSION_TYPE`），不要直接改业务代码。
- Linux 侧没有 `verify-hotkey.ps1` 的等价物。CI 的 `shell-linux` job 只做编译、
  clippy 与 lib 单测；GUI 行为仍需在真实桌面人工确认。
- 打开外部路径/链接走 `platform::open_path` / `platform::open_url`（`xdg-open`）。
  某个桌面没装 `xdg-open` 时，这些入口会报错，热键与本地记录不受影响。

## 安全与配置

- capability 只授三项：`core:event:allow-listen`、`core:event:allow-unlisten` 与
  `core:window:allow-start-dragging`。前两项供唤出事件订阅；少了第三项，无边框窗口
  **看起来正常但完全拖不动**。加权限前先在 `ui/src` grep 到真实调用；自定义命令由
  本地 origin 直接调用，不需要 capability。
- 写剪贴板也走 Rust 侧（`copy_diagnostics` 经 `ClipboardExt`），所以 capability 里没有
  `clipboard-manager:*`。能少给一项能力就少给一项。
- 不把密钥、机器相关路径写进仓库；示例配置用占位值。
