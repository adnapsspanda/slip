# Repository Guidelines

Slip 是一个 Windows 11 桌面便签式待办工具：热键唤出捕获框，输入即落进本地
`inbox.md`，桌面上一张纸就能勾掉。核心设计见
`docs/design/Slip-架构与模块定义.md`（**唯一真源**，实现与文档冲突时先改文档）。

## Project Structure

```
crates/slip-domain/   纯领域：文本规范化、拆分、状态转换、ID/时间格式闭包。零 I/O。
crates/slip-store/    唯一的磁盘写入者：Markdown 解析/渲染/局部替换 + 事务层 + 偏好。
crates/slip-app/      命令队列、消费者、presenter、UiSnapshot。**不含 tauri**，可纯单测。
src-tauri/            Tauri 外壳：窗口属性、快捷键、IPC 边界。薄，不放业务状态。
ui/                   Svelte 5 + Vite 前端；`bun run build` 产物进 `ui/dist` 后被嵌入。
tools/                端到端验收脚本（见下）。
docs/design/          架构与模块定义（v11 及其后的修订）。
docs/research/        外部项目调研。
```

依赖方向**只能**是 `src-tauri → slip-app → slip-store → slip-domain`，反向依赖一律拒绝。

### 权限最小化

`src-tauri/capabilities/default.json` 只授两项：`core:default` 与
`core:window:allow-start-dragging`。这是审计结果，不是偷懒：前端实际只用
`core.invoke`（自定义命令）与 `event.listen`（唤出事件），两者都在 `core:default` 里；
窗口显隐/聚焦、全局热键、开机自启全部由 Rust 侧直接调用插件 API，**不经过 webview IPC**。

**`core:default` 不包含 `allow-start-dragging`**（它的组成是
`core:window:default` + `core:webview:default` + `core:app:default` + …，而
`core:window:default` 只有只读类命令）。所以标题栏的 `data-tauri-drag-region` 必须
单独授 `core:window:allow-start-dragging`，否则**无边框窗口完全无法移动**——而外观上
一切正常，只有真的去拖才会发现。收窄权限时踩过这个坑，现已由验收脚本钉住
（断言 `plugin:window|start_dragging` 不被拒）。

加权限前先确认前端真的有对应调用（在 `ui/src` 里 grep 得到），否则等于把能力白送给
一段本该做不到这些事的代码。改完必须跑 `tools/verify-hotkey.ps1`——它验证了
事件订阅（唤出后捕获框获得焦点）与 IPC 是否仍然可用，是这条收窄唯一可靠的守门人。

## Build, Test, and Development

本机 `cargo` 不在 PATH，需先补：

```powershell
$env:PATH = "C:\Users\spand\.cargo\bin;" + $env:PATH
```

| 目的 | 命令 |
|---|---|
| 全量测试 | `cargo test --workspace --offline` |
| 性能测量 | `cargo test --release -p slip-store --test perf --offline -- --ignored --nocapture` |
| 静态检查 | `cargo clippy --workspace --all-targets --offline -- -D warnings` |
| 格式 | `cargo fmt --all` / `cargo fmt --all --check` |
| 跑外壳 | `cargo run -p slip-shell` |

- 依赖已缓存，**优先加 `--offline`**；新增依赖时去掉它一次。
- `cargo test` 的纯领域/存储/调度测试必须在 1 秒内跑完；超过说明有人偷偷加了 I/O。
- **性能测试全部 `#[ignore]`**，不进默认测试；`perf.rs` 会打印构建配置，
  引用数字时必须连配置一起引用（debug 与 release 在解析上差 3.5 倍）。
- 运行时日志走 `tracing` 到 stderr，级别用 `SLIP_LOG=error|warn|info|debug|trace`
  覆盖，默认 `info`。stdout 只留 `[slip]` 开头的启动摘要。
- 环境变量 `SLIP_VAULT` 可指定 vault 目录（默认 `%USERPROFILE%\Documents\Slip`），
  用于测试与多实例并行。
- 前端是 Svelte 5 + Vite，用 **bun**（`C:\Users\spand\.bun\bin\bun.exe`，不在 PATH）。
  `ui/dist` 在**编译期**被嵌入二进制，所以改完界面必须重新构建：

  ```powershell
  cd ui; bun run build; cd ..
  cargo run -p slip-shell
  ```

  本项目不依赖 `tauri` CLI：没有 `beforeDevCommand` / `devUrl`，就是为了让
  裸 `cargo` 能跑通。要加 `tauri dev` 时再补那两项。
- `ui/dist` 与 `node_modules` 不入库（见 `.gitignore`）。

## 产品形态：召唤式

窗口默认**隐藏**（`tauri.conf.json` 的 `visible: false`），托盘是唯一常驻入口，
热键唤出、记完或 Esc 收走。三条不能破坏的约定：

1. **手动双击必须看得见窗口。** 自启插件带 `--autostart` 标记参数，只有带这个参数
   才静默驻留；否则 `lib.rs` 在 setup 里调一次 `hotkey::show_on_launch`。改掉这条
   会让用户双击 exe 后"什么也没发生"。
2. **唤出要贴着光标**，放在光标右下 16 逻辑像素；放不下就翻到左上，最后夹紧进
   `Monitor::work_area()`。任何信息拿不到都退化为居中，**绝不能让窗口跑丢**。
3. **失焦自动收走，但只对"召唤"上膛**，且有 400ms 宽限期。
   - 热键与托盘走 `summon`：上膛，点别处就收走。
   - **启动展示走 `show_on_launch`：不上膛。** 用户是明确打开了程序，别的常驻应用
     抢前台不该让窗口消失。本机实测 Clash Verge 每 2.7~4.5 秒抢一次前台，上膛的
     结果就是"启动后一个窗口都没有"。启动出来的窗口靠 `Esc`、托盘或热键收走。
   - 400ms 宽限期：Windows 前台锁定可能让 `set_focus()` 失败并立刻送来
     `Focused(false)`，没有宽限期就会变成"一闪就没"。
   - 另一条判据：**从未被聚焦过的窗口不因"失焦"收走**——那是"还没拿到焦点"，
     不是"用着用着离开了"。

热键存在 `<vault>/ui-state.json` 的 `hotkey` 字段，由 `hotkey::resolve_shortcut`
解析，解析失败回落默认值。**`PaperViewSet` 必须原样携带这个字段**：`Prefs` 是整体写的，
视图层少带一个字段就等于每次拖拽都静默擦掉用户的配置（回归测试
`view_roundtrip_never_drops_the_hotkey` 锁的就是这条）。

## 端到端验收

### 先决条件：必须在**交互桌面**的终端里启动

如果你的终端本身运行在沙盒 / 隔离环境里（某些 agent 工具的终端就是这样），从这里
`cargo run` 出来的进程会建在**隔离的窗口站/桌面**上：

- 进程活着、WebView2 正常起来、配置里的一切都成功；
- 但**用户看不到窗口**，外部 `EnumWindows` 该进程的顶层窗口数是 **0**；
- 且 `cursor_position()` 读不到光标，`place_near_cursor` 永远退化到 `center()`。

指纹很好认：**多次启动的窗口坐标完全相同（都是屏幕正中），并且 `focused: false`**；
在交互桌面上启动时坐标会跟随光标变化、`focused: true`。

遇到"进程在跑但没窗口"，先跑 `pwsh -File tools/diag-window.ps1`，
再确认启动它的终端是不是普通交互终端。这不是程序缺陷。

### 同一类沙盒症状的第二种表现（窗口存在但不显示）

沙盒有时不表现为"没有窗口"，而是**窗口存在、`WS_VISIBLE` 为假，但 Tauri 自己
报告 `visible=true`**（它返回的是自己缓存的状态，不是 Win32 真值）。此时：

- `EnumWindows` 能找到 `Tauri Window`，`IsWindowVisible` 却是 `false`；
- 验收脚本里所有"窗口可见"断言全红，而应用日志写着 `visible=true focused=true`。

判定方法（本会话实测过）：让 **explorer.exe** 启动同一个 exe——
`& explorer.exe .\target\debug\slip-shell.exe`。若那样启动窗口 `vis=True`，
就是父进程沙箱的问题，代码没坏。

**不要在沙盒里跑验收后就断定"改动破坏了窗口"。** 先拿上一版提交（`git stash` 后
重编）跑同一条断言：如果它也红，问题在环境。

改动窗口、热键、或捕获链路之后，**必须**跑一次：

```powershell
pwsh -File tools/verify-hotkey.ps1
```

它用外部视角取证而非"看起来对"：Win32 `keybd_event` 发**真实**的
`Ctrl+Alt+Space`、`GetWindowRect`/`IsWindowVisible` 读窗口状态、CDP 读界面焦点与
列表、读 `inbox.md` 字节。覆盖面（**条目数会随功能增加，不要在这里写死**）：
自启静默 / 手动启动可见且不被抢焦点收走 / 热键唤出到光标旁 / 放不下时翻转 /
单屏与负坐标副屏的摆放 / 捕获全链路 / IME 组合态 / 超长拦截 / 行内编辑与锁定 /
归档与撤销 / 失焦自动收走 / 磁盘取证。每项有 `[PASS]/[FAIL]/[SKIP]`，
失败时退出码为失败项数。

**断言不得依赖未声明的环境前提。** 已经因此假红过两次：一次是按窗口类名找窗口，
而 `Tauri Window` 被 Clash Verge 共用；一次是把光标坐标写死，显示器数量一变
"右下放不下"的前提就不成立。坐标要由 `PrimaryScreen.Bounds` 推导，窗口要按进程 ID 找。

### 写验收脚本时踩过的坑（会再踩，先看这里）

- **`.ps1` 文件必须带 UTF-8 BOM。** 中文 Windows 上不带 BOM 会被按 ANSI/GBK 解码，
  中文注释变乱码并直接引发语法错误。`edit`/`write` 工具写回时会丢 BOM，改完要补：

  ```powershell
  $p = "tools/verify-hotkey.ps1"
  $t = [System.IO.File]::ReadAllText($p, [System.Text.UTF8Encoding]::new($false))
  [System.IO.File]::WriteAllText($p, $t, [System.Text.UTF8Encoding]::new($true))
  ```

- **PowerShell 把 `$null` 传给 `[string]` 形参时会变成空字符串。**
  `FindWindow($null, "Slip")` 实际是"找类名为空的窗口"，永远返回 0，
  而 `IsWindowVisible(0)` 返回 false——于是所有可见性断言都会诡异地"通过"或失败。
  要传真正的 NULL 必须用 `[NullString]::Value`。

- **PowerShell 是 DPI 不感知进程。** `GetWindowRect` 与 `GetCursorPos` 都被系统按
  同一比例虚拟化（本机 150%），所以两者可以直接相减比较，但**屏幕尺寸也是虚拟后的
  数值**（1600 物理 → 1066 虚拟），断言里不能拿物理值算翻转/越界。

**截图不能作为透明的证据**：DirectComposition 窗口的透明区在 `BitBlt` 截图上会渲染成
近黑，无法区分"真透明"与"透明失效"。那一项只能人眼判定，见设计文档 §9.7.1。

## 状态生命周期（这一族 bug 都会伪装成"功能坏了"）

本项目反复出现同一类缺陷：**某个状态在新条件下失去意义，但没有谁负责清理它**。
症状一律是"静默的错误信息或毫无反应"——不报错、不崩溃、测试也常常照样通过。

写新功能时按这张表对照一遍：**新增一个有生命周期的状态，必须同时回答"它何时失效、
谁负责清、漏了会怎样"。**

### 前端（`ui/src`）

| 状态 | 有效范围 | 必须在何时清 | 漏掉后的症状 |
|---|---|---|---|
| `filter` / `filtering` | 当次浏览会话 | `summon()` | 再唤出落在一张被筛过的列表上，像"东西没了" |
| `showArchived` | 当次浏览会话 | `summon()` | 再唤出仍停在归档抽屉 |
| `showBackups` / `backups` / `pendingRestore` | 当次面板打开期间 | `summon()`、面板关闭、恢复成功 | 残留的"确认？"状态，点下去恢复的是旧选择 |
| `notice`（带撤销按钮） | 一次操作上下文 | `summon()`、`reload`、`isolate`、`restore` | 按钮还在但后端已无对应撤销，点了一下没反应 |
| `editingId` / `session` / `draft`（SlipList 内） | 一次编辑会话 | 该条目从 `slips` 里消失时 | `editing` 恒为 true → App 的 `Ctrl+Z` 被永久拦掉，而界面上根本没有编辑框 |
| `inFlight` / `draftVersion` | 单次提交 | 提交结束 | 迟到的 ACK 把新草稿误判成自己的 |

### 后端（`slip-app/dispatch.rs`）

| 状态 | 有效范围 | 必须在何时清 | 漏掉后的症状 |
|---|---|---|---|
| 撤销栈 | **当前文档** | 任何"内容被整体替换"的路径：`Reload`、`RestoreBackup`、`IsolateAndReset` | 逆操作指向已不存在的条目 → 报一句自相矛盾的错，且失败会把该步放回栈里，之后每次撤销都撞同一堵墙 |
| `needs_reload` | 一次外部修改 | 成功写入或成功 `reload` | 永远写不进去 |

### 判据

1. **"整体替换内容"的路径必须清空一切基于旧内容的派生状态。** 加一条新路径时，
   先搜一遍 `Command::` 枚举，把所有同类入口列出来逐个核对，而不是只修手上这一处。
2. **临时视图状态不跨唤出。** 唤出 = 新一次捕获会话，不继承上一轮的浏览位置。
3. **有条件渲染的编辑框，其状态必须随数据消失而失效。**
4. **当前实际覆盖**（不要假设"表里写了就等于测了"）：

   | 条目 | 覆盖方式 |
   |---|---|
   | 过滤状态跨唤出 | 端到端「再唤出后过滤条被收起」 |
   | 条目消失后的编辑态 | 端到端「条目消失后编辑框也被清掉」 |
   | 撤销栈 + `Reload` | 集成测试 `reload_clears_the_undo_stack` |
   | 撤销栈 + `RestoreBackup` | 端到端「恢复需要二次确认」间接覆盖（恢复后撤销栈已空） |
   | 撤销栈 + `IsolateAndReset` | **没有专门断言**（触发它需要先构造降级态） |
   | `showArchived` / `showBackups` 跨唤出 | **没有断言**，靠本表人工核对 |
   | `notice` 跨唤出 | 端到端「再唤出后撤销提示已过期清除」 |
   | `notice` 与三个整体替换路径 | **没有断言**，靠本表人工核对 |

   新增/修改这些路径时，能补断言就补；补不上的（例如需要构造降级态、或要清空整个列表
   才能触发），在 `docs/ralph-backlog.md` 里写明"为什么没覆盖"，不要默默留空。

## 质量门禁

三条门，从便宜到贵：

| 门 | 命令 | 覆盖 |
|---|---|---|
| 提交前钩子 | `git commit` 自动触发 | 编码（`*.ps1` 的 BOM）/ PowerShell 语法 / prettier / fmt / clippy / test |
| CI | `.github/workflows/ci.yml` | 同上 + 前端构建 + release 构建 |
| 本机验收 | `pwsh -File tools/verify-hotkey.ps1` | 真实热键 / 托盘 / 窗口定位 |

**每个克隆要装一次本地钩子**（`core.hooksPath` 是本地配置，不随仓库走）：

```powershell
git config core.hooksPath .githooks
```

临时跳过用 `git commit --no-verify`，但 CI 仍会拦住。

工具链由 `rust-toolchain.toml` 钉死（1.99.1 + clippy + rustfmt）。**不要随手升级**：
同一份代码在更新的 rustc 上会因为新 lint 而"突然失败"，而失败原因与你的改动无关。

CI 只跑 Windows：这个应用依赖 Win32 托盘、全局热键与 WebView2，在 Linux 上连编译都过不去，
假装跨平台只会给出假的绿灯。E2E 也**不**在 CI 里——它需要交互式桌面会话和真实光标。

### 干净克隆的硬顺序

`ui/dist` 不入库，而 `tauri::generate_context!` 在**编译期**读它。所以任何 cargo 命令
之前必须先构建前端，否则会得到一句毫无指向性的 `proc macro panicked`：

```powershell
cd ui; bun install; bun run build; cd ..
cargo build -p slip-shell
```

`src-tauri/build.rs` 里加了前置检查，缺产物时会直接打印该跑什么命令。

## Coding Style

- Rust 2024 edition，`cargo fmt` 默认风格，不允许手写对齐。
- 类型 PascalCase，函数 `snake_case`，跨 crate 的契约类型保持 `pub` 且**字段命名不改**。
- 注释写"为什么"，不写"做了什么"。凡是被某条 v11 条目约束的代码，注释里标出编号（如 `§4.7`）。
- 单元测试放文件底部 `#[cfg(test)] mod tests`；跨 crate 验收放 `tests/`。

## Testing Guidelines

- 领域层与调度层用**纯内存**测试，禁止碰真磁盘。
- 事务层与格式闭包必须用**真实临时目录 + 真实文件**（`tests/acceptance.rs`），
  因为这些测试要验证的正是"磁盘上到底发生了什么"，mock 掉磁盘等于没测。
- 测试名要说清被验证的行为，例如 `noop_patch_does_not_rewrite_the_file`。
- 修 bug 必须同时补一条能复现的回归测试。

## 不可回退的约束

以下都是经过推演和测试锁定的，改动前先读设计文档对应小节：

- 未修改行**逐字节保留**；候选文档只能由原始行数组局部替换构造，禁止全文重序列化。
- 提交点 = `rename` 成功。之后的失败只能记 `DurabilityUnconfirmed`，不得报失败。
- 单队列单消费者：`presenter` 由消费者独占，不引入锁或版本号过滤。
- `ui_rev` 在**每一次**生成并发布的快照上递增，包括视图事件；`GetSnapshot` 不递增。
- 禁止引入：文件 watcher、内容去重、回调/扩展点、事件总线、SQLite 索引、
  物理归档目录、跨字段或时钟合法性校验。

## Security & Configuration

- 不把密钥、机器相关路径写进仓库；示例配置用占位值。
- Tauri capability 只授予确实用到的权限（当前 `core:default` + `core:window:allow-start-dragging`）。加权限前先确认前端真的有对应调用。
- 磁盘写入只允许经由 `slip-store`；外壳与前端都不得直接读写文件。
