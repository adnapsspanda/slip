# Repository Guidelines

> **第一次接手？先读 [`docs/handover.md`](docs/handover.md)**：里面写了什么能跑、
> 什么被 Smart App Control 挡住、以及**哪些绕法已经被证明无效（别再试）**。


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
   | 撤销栈 + `IsolateAndReset` | 集成测试 `isolate_and_reset_clears_the_undo_stack`（先构造降级态；已做变异验证：去掉 `undo.clear()` 该测试会红） |
   | `showArchived` / `showBackups` 跨唤出 | **没有断言**，靠本表人工核对 |
   | `notice` 跨唤出 | 端到端「再唤出后撤销提示已过期清除」 |
   | `notice` 与三个整体替换路径 | **没有断言**，靠本表人工核对 |

   新增/修改这些路径时，能补断言就补；补不上的（例如需要构造降级态、或要清空整个列表
   才能触发），在 `docs/ralph-backlog.md` 里写明"为什么没覆盖"，不要默默留空。

### 环境陷阱：crates.io 被本地代理劫持

本机（2026-11-11）实测：`cargo search` / 新增依赖失败，报
`SSL connect error (schannel: failed to receive handshake)`，但**不是断网**——

```
https://www.baidu.com       → 200
https://www.microsoft.com   → 200
https://github.com          → 失败
crates.io                   → 解析到 198.18.1.197
```

`198.18.0.0/15` 是保留的基准测试网段，Clash 一类的本地代理会用它做"假 IP"透明劫持，
再自己转发。这台机器上代理没转发成功，于是 TLS 握手对着本地端口失败。

**影响**：任何需要新增 crate 的轮次（本项目里是 R03/R08/R28/R29 的
`tauri-plugin-notification` / `tauri-plugin-clipboard-manager`）都无法进行——
与"要不要加权限"无关，是拿不到依赖包。

**加依赖前的检查**：

```powershell
cargo search tauri-plugin-notification --limit 1   # 能返回结果才继续
Resolve-DnsName crates.io -Type A                  # 若解析到 198.18.x.x，说明走了代理
```

修法是用户侧的事：让代理正常工作、或临时关掉代理直连、或配置
`~/.cargo/config.toml` 的 `[source]` 指向可用镜像。

### 环境变化记录：升级 Windows 11 专业版之后（2026-11-11 晚）

升级到 **Windows 11 专业版（Build 26300）** 后实测：

- **SAC 策略没有因此改变**：`VerifiedAndReputablePolicyState` 仍是 `1`（强制），
  `CodeIntegrityPolicyEnforcementStatus` 仍是 `2`；
- 但**实际可用性明显改善**：

    | 项 | 升级前 | 升级后 |
    |---|---|---|
    | `cargo check --tests` | 持久失败（build script 被拒） | **通过** |
    | `cargo build -p slip-shell` | 时好时坏 | **通过** |
    | 端到端 `verify-hotkey.ps1` | 时好时坏 | **104/104** |
    | 单元测试 | 多数被拒 | **9 个目标逐个执行全部通过，合计 225 项** |
    | 一次性 `cargo test --workspace` | 中断 | **仍会随机被拦掉一个二进制**（216 项后中断） |

- 结论：**别指望升级系统版本或搬 Dev Drive 来"关闭"这个拦截**（策略状态没变）。
  真正的变化是"被拦的产物随时间会被放行"，所以**逐目标执行 + 重试**才是最稳的跑法；
  `cargo test --workspace` 一次跑完仍可能被随机拦，**失败时先怀疑这个，别怀疑代码**。

### 环境陷阱：`os error 4551`（应用控制策略阻止执行）

**症状**：`cargo test` / `cargo clippy` 或直接启动产物时报
`应用程序控制策略已阻止此文件。 (os error 4551)`，被点名的通常是
`target/debug/deps/` 下某个 exe。

**先跑自检**：`pwsh -File tools/check-environment.ps1`（加 `-Probe` 会真的启动一次产物）。
它报告 SAC 状态、生效策略、产物的拦截记录并给出建议；退出码 1 = 产物不可运行。
PowerShell 自身是微软签名的，不受这个策略影响，所以这条自检永远可用。

本机（2026-11-11）的定论：拦截来自 **Smart App Control 本体**——

```
Policy ID: 0283ac0f-fff1-49ae-ada1-8a933130cad6     （自身即 Base 策略）
好记的名称: VerifiedAndReputableDesktop
平台策略: true    策略已签名: true    当前强制执行: true
拒绝理由: did not meet the Enterprise signing level requirements
```

`CiTool -lp` 里其余 `.cip` 都是微软内置（驱动策略、VBS 策略、未启用的 Evaluation/
Lockdown 变体），没有企业自定义策略。

**被拦的永远是"被加载的那份产物"，工具链从来不是目标。** 这一点必须说清，因为
事件日志的措辞极易误读：

```
a process (…\rustup\…\bin\cargo.exe) attempted to load
  …\target\debug\deps\slip_shell-46ad4e26a133e24c.exe that did not meet …
```

`cargo.exe` 只是**加载者**，它自己被拒的是那份测试二进制。实测 200 条拦截事件里，
`cargo` / `rustc` / `rustup` / `rustfmt` / `clippy` 作为被加载对象出现 **0 次**；
被拦的全是 `slip_app-*` / `slip_domain-*` / `slip_shell-*` / `acceptance-*` /
`frontend_contract-*`。

由此推出两条实操结论：

- **重装 Rust 工具链没有意义**——问题从来不在 cargo 身上；
- 直接跑 `cargo --version` / `cargo build` 永远正常，`cargo` 只是"启动产物"那一步
  会被拒。

**为什么同一份产物时好时坏**：SAC 按**文件**问云端信誉（ISG）。对未签名的新产物，
一次查询可能返回"暂时拒绝"，过一会儿再查又变成"放行"。实测：`slip-shell.exe`
在 17:45 被拒，17:57 同一份文件却能启动。

**注意：build script 也是可执行文件，同样会被拒。** 现象是
`error: failed to run custom build command for slip-shell` 并伴随 `os error 4551`——
连 `cargo check` 都不"纯静态"。实测 4/4 次复现；而 `cargo clippy` 因为复用了缓存里的
build script 输出，仍然能通过。判据：`fmt` / `clippy` 通过**不代表** `check` 能通过。

**标准应对：重试同一个二进制，不要删了重编。**

```powershell
pwsh -File tools/cargo-test-retry.ps1   # 只对 4551 重试，且默认不删产物
```

删掉重编是**帮倒忙**：每次重编都是一个新文件，又要重新经历"首次执行被拒"。
（这条是实测纠正过来的，脚本早期版本恰好是删了重编。）

其它已经准备好的应对：

- 提交钩子遇到"只有 4551、没有 `test result: FAILED`"时**放行但大声警告**，
  把"本次提交未经完整测试验证"打在输出里；判据严格，二者缺一不放行；
- 端到端验收用已构建的 exe，通常仍可跑；碰到拒绝就再启动一次；
- 实在跑不了的，如实记进本轮记录并补跑——**不要把没跑过的写成通过**。

**SAC 没有白名单**，以下做法全部无效，别浪费时间：

| 尝试 | 为什么无效 |
|---|---|
| 自签名证书 + `signtool` / `Set-AuthenticodeSignature` | SAC 看云端信誉（ISG），不看本地证书链 |
| 加补充 WDAC 策略放行 `target\**` | SAC 是签过名的平台策略，不接受用户补充策略 |
| Defender 排除目录 / 关实时保护 / 改 SmartScreen | 都不是代码完整性机制 |
| 换目录 / 改文件名 / 重装工具链 | 判定按文件内容与签名，与路径和工具链无关 |

**日常开发不必关 SAC**，重试即可。只有当你希望"构建产物一次就能运行"（例如要反复
手动点开 GUI）时，才需要考虑：关闭 SAC（单向开关，关掉后再打开需重置系统）、换到
没有 SAC 的 Windows 环境（Windows Sandbox / Hyper-V）、或换一台机器。

**WSL 帮不上**：它能让 188 项（domain/store/app）测试跑起来，但 GUI 与端到端需要
Windows 窗口、全局热键与 WebView2；而且拦截发生在 **Windows 执行 exe 时**，
在 WSL 里交叉编译出来的 Windows exe 拷回去照样会被拦。

## 在 WSL 里跑纯逻辑 crate（推荐做法）

`slip-domain` / `slip-store` / `slip-app` 三个 crate 明确不含 tauri，可以在 Linux 上编译
运行——**完全绕开 Windows 的 Smart App Control**（本机反复踩到的坑：新编出的产物会被按
文件拒绝执行）。

```powershell
# Windows 侧调用即可
wsl.exe -d Arch -- bash -lc 'sh /mnt/d/Dev/simple-todo/tools/wsl-test.sh'
```

- 脚本把 `CARGO_TARGET_DIR` 指到 WSL 的 ext4（源码在 `/mnt/d` 的 9p 上，产物放 Linux 侧快得多）；
- 依赖：本机已把 Windows 的 `~/.cargo/registry` 复制进 WSL，因此默认 `--offline`；
  WSL 网络不便时也无需联网。要联网加 `--online`；
- **平台相关的测试必须加 `#[cfg(windows)]`**：原生错误码（如 32/33 = 共享冲突、
  5 = 拒绝访问）在 Linux 上含义完全不同。`sharing_violation_maps_to_busy` 曾因此在 WSL 里红过。

**仍然只能在 Windows 上做的**：`slip-shell`（tauri + WebView2）的构建与
`tools/verify-hotkey.ps1`（真实热键、窗口、托盘）。

## 质量门禁

三条门，从便宜到贵：

| 门 | 命令 | 覆盖 |
|---|---|---|
| 门禁对等性 | `pwsh -File tools/check-ci-parity.ps1` | 钩子与 CI 是否覆盖同一组关口（零漂移） |
| 提交前钩子 | `git commit` 自动触发 | 编码（`*.ps1` 的 BOM）/ PowerShell 语法 / prettier / fmt / clippy / test |
| CI | `.github/workflows/ci.yml` | 同上 + 前端构建 + release 构建 |
| 本机验收 | `pwsh -File tools/verify-hotkey.ps1` | 真实热键 / 托盘 / 窗口定位 |

**门禁对等性（R76）**：`tools/check-ci-parity.ps1` 把钩子与 CI 的步骤按关键词归一到同一组
关口再比对。做成"归一 + 白名单"而不是逐字比对——两边措辞本来不需要一致，逐字比只会制造噪音，
而噪音会让守卫被关掉。CI 独有（Release 构建、安装 bun、缓存）走白名单。

钩子还有一个曾经的坑：它**只在 `ui/dist` 缺失时**才构建前端，而 CI 每次都构建。于是
"改了 `ui/src` 但 dist 还在"时，钩子会拿**旧产物**跑 prettier / 纯逻辑 / 体积三项检查。
现在改为"缺失或源码比产物新"都重建。

**每个克隆要装一次本地钩子**（`core.hooksPath` 是本地配置，不随仓库走）：

```powershell
git config core.hooksPath .githooks
```

临时跳过用 `git commit --no-verify`，但 CI 仍会拦住。

工具链由 `rust-toolchain.toml` 钉死（1.99.1 + clippy + rustfmt）。**不要随手升级**：
同一份代码在更新的 rustc 上会因为新 lint 而"突然失败"，而失败原因与你的改动无关。

CI 的**门禁主体**只跑 Windows：外壳依赖 Win32 托盘、全局热键与 WebView2，在 Linux 上连编译都
过不去，假装整个应用跨平台只会给出假的绿灯。E2E 也**不**在 CI 里——它需要交互式桌面会话和真实光标。

但**三个逻辑 crate（domain / store / app）确实与平台无关**（`slip-domain` 零 I/O、`slip-app` 无 GUI
依赖），所以 CI 里额外有一个 `逻辑层（Linux）` job 跑它们的测试与 clippy。这不是装样子：它会在
"有人把 Win32 调用写进 store"的第一时间变红。两边的边界必须一直清楚——**外壳 Windows，逻辑跨平台**。

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

### 断言写完要**变异验证**一次

**操作纪律（血泪）**：

1. **先确认「现在能跑」，再动手改**。环境可能在几分钟内从"能跑测试"变成"任何新产物都被拒"
   （本机是 Smart App Control，见下面的环境陷阱）。先跑一次基线，跑不动就别开始。
2. **改完立刻还原**，不要跨步骤、更不要跨轮次保留变异。本轮就发生过：注入变异后
   测试被环境拦住，六次重试都没放过，只能放弃验证并立刻还原——如果忘了还原，
   仓库里就留下了 `if false`。
3. **还原后必须核验**：`git status` 应为空，且被改的那几行要肉眼复看一遍
   （脚本替换可能没匹配上，却仍然打印"restored"）。
4. 一次只改一处，否则红了也不知道是哪一处生效。


"测试通过"不是证据——它只说明在**当前实现**下测试没红。"去掉修复后测试会红"才是。
本项目已经因此抓到过一条永远绿、却什么都没守住的断言（见 backlog 的 R24 记录）。

做法很简单：把修复临时改掉（把常量改成不可能成立的值、把清理语句注释掉、让函数直接
返回原值），跑那条测试，确认它**变红**，然后还原。两次 `cargo test -p <crate> --lib
<过滤词>` 就够，不必跑整套。

已做过变异验证的守卫（改动这些地方时请重新验证）：

| 守卫 | 变异方式 | 结果 |
|---|---|---|
| `retry_rides_out_a_short_lock` | `ATTEMPTS: 5 → 1` | FAILED ✅ |
| `strips_common_list_prefixes` 等 5 条 | `strip_list_prefix` 直接返回原行 | 5 条 FAILED ✅ |
| `sharing_violation_maps_to_busy` | `if false && (code == 32 \|\| code == 33)` | FAILED（left: Other）✅ |
| 端到端「再唤出后过滤条被收起」 | 删掉 `summon()` 里的 `filtering = false` | FAILED（filterOpen=true）✅ |
| `counts_code_points_not_bytes`（字数按码点） | `text.chars().count()` → `text.len()` | FAILED ✅ |
| `bom_is_detected_and_written_back_byte_identically` | 删掉 `render` 里的 BOM 分支 | FAILED ✅ |
| `a_real_change_drops_the_bom` | 提交时保留 BOM | FAILED ✅ |
| `validate_candidate` 的守卫 | 改成无条件 `Ok(())` | FAILED ✅ |

**尚未**做变异验证的：`isolate_and_reset_clears_the_undo_stack` 已做过（见 R24）；
其余前端断言、以及所有只靠"跑绿了"确认的测试，都还没验证过。


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

### 这些约束由谁守着

"写在文档里"不等于"有人守"。逐条列出当前的守护方式，新增约束时按同一格式补一行：

| 约束 | 守护方式 | 能不能被机器发现 |
|---|---|---|
| 未修改行逐字节保留 | `format_closure.rs` 5 条往返测试 + `real_pipeline_appends_and_preserves_untouched_lines` | 能 |
| 提交点 = `rename`；之后只记 `DurabilityUnconfirmed` | `commit_ack_survives_backup_failure`、`commit_ack_survives_directory_sync_failure`、`sync_dir_failure_is_observable_after_commit` | 能 |
| `ui_rev` 每次递增 / `GetSnapshot` 不递增 | `accept_increments_ui_rev_even_when_content_rev_is_unchanged`、`current_does_not_bump_ui_rev` | 能 |
| 单队列单消费者、`presenter` 不上锁 | `tools/check-forbidden.ps1` 查 `Mutex<Presenter>` / `RwLock<Presenter>` | 能（Grep 级） |
| 禁止引入 watcher / SQLite / 物理归档 | `tools/check-forbidden.ps1`（查依赖与 `archive.md`） | 能（Grep 级） |
| 前端不注入 HTML | `tools/check-forbidden.ps1` 查 `{@html`（跳过注释） | 能 |
| 前端不得发起网络请求 | 同上，查 `fetch` / `XMLHttpRequest` / `WebSocket` / `sendBeacon` / `EventSource`（跳过注释） | 能 |
| 不把字符串当代码/标记执行 | 同上，查 `eval` / `new Function` / `innerHTML=` / `insertAdjacentHTML` / `srcdoc=` | 能 |
| CSP 不得被削弱 | 同上，查 `unsafe-inline` / `unsafe-eval` / `"csp": null` | 能 |
| **禁止内容去重 / 回调扩展点 / 事件总线 / 跨字段与时钟校验** | **没有自动守护** | **不能**——它们不是文本模式，只能靠评审时对照本节 |

**自检模式**：`pwsh -File tools/check-forbidden.ps1 -SelfTest` 会拿正负样本跑一遍三条新模式——
防止正则写错导致守卫**永远是绿的**（那种坏法最难发现）。整脚本实测约 260 ms；
它曾经是 4.2 秒（每个检查各自递归扫全仓库、且 `Mask-Comments` 逐字符 `Substring` 是 O(n²)），
R75 一并修掉了。

`tools/check-forbidden.ps1` 同时接在提交钩子与 CI 里；它被设计成**只报文本证据**，
查不出的绝不假装覆盖（例如"禁止全文重序列化"要靠往返测试，不在里面）。

## Security & Configuration

- 不把密钥、机器相关路径写进仓库；示例配置用占位值。
- Tauri capability 只授予确实用到的权限（当前 `core:default` + `core:window:allow-start-dragging`）。加权限前先确认前端真的有对应调用。
- **写剪贴板也走 Rust 侧**（`copy_diagnostics` 命令经 `ClipboardExt`），webview 只拿 `core.invoke`——所以 capability 里**没有** `clipboard-manager:*`。能少给一项能力就少给一项。
- **`inbox.md` 的读写只允许经由 `slip-store`。** 外壳可以碰两类东西：它自己产出的
  附件（`export-YYYYMMDD-HHmmss.md`）与只读的目录枚举（损坏证据 `inbox.md.corrupt-*` 的名字与时间）。
  外壳**不得**直接读写 `inbox.md` 的内容——读它要走 `slip_store::peek_slips`，
  写它只能经 `Store` 的方法。前端一律不碰文件系统（它连路径都拿不到，只有 `vault_path`
  这一个字符串用于显示）。
