# Slip

极轻量的召唤式待办捕获工具。按一个热键，把脑子里那点琐碎念头丢进去，然后继续手头的事。

Windows 是主平台；Linux 外壳已支持（首版以 **X11** 为准，见文末已知限制）。

不打断、不整理、不要求你先想清楚它属于哪个项目。

---

## 快速开始

需要 [Rust](https://rustup.rs) 和 [bun](https://bun.sh)。

### Windows

```powershell
# cargo 不在 PATH 时先补（按你的用户名替换，或直接用下面这行）
$env:PATH = "$env:USERPROFILE\.cargo\bin;" + $env:PATH

# 1. 构建前端（产物会被嵌进二进制，改了界面就必须重建这一步）
cd ui
bun run build
cd ..

# 2. 构建并运行
cargo run -p slip-shell
```

### Linux

先装 Tauri v2 的系统依赖，再构建前端与外壳：

```bash
# Arch
sudo pacman -S --needed base-devel webkit2gtk-4.1 gtk3 libappindicator librsvg patchelf openssl

# Debian / Ubuntu
sudo apt install libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev \
  librsvg2-dev patchelf libssl-dev

cd ui
bun install
bun run build
cd ..

cargo build -p slip-shell
./target/debug/slip-shell          # 默认 vault：$HOME/Documents/Slip
# 需要隔离测试时：SLIP_VAULT="$HOME/Slip-dev" ./target/debug/slip-shell
```

需要在图形会话里运行（首版以 X11 为准）。只验证编译/单测不需要桌面：
`cargo build -p slip-shell`、`cargo test -p slip-shell --lib`。

开机自启时程序静默驻留托盘；**手动双击（或 `cargo run`）会直接把窗口显示出来**，
不会因为别的程序抢焦点而消失——它是个正常窗口，用 `Esc` 或托盘收走。
平时按 `Ctrl+Alt+Space` 唤出。

日常开发可以加 `--offline`（依赖已缓存）。

---

## 日常使用

窗口是召唤式的：热键叫出来，记完自己消失。

| 操作 | 效果 |
|---|---|
| `Ctrl+Alt+Space` | 唤出纸片，光标已在输入框里 |
| `Enter` | 收录这一条，窗口自动收起 |
| `Shift+Enter` | 换行（一条待办可以写多行）；粘贴带 `- ` / `1. ` 这类清单前缀会自动剥掉 |
| `Esc` | 收起窗口，放弃这次输入 |
| 点纸条左侧方框 / 列表里按 `Space` | 标记完成 / 取消完成 |
| 双击纸条正文 / 列表里按 `Enter` | 进入编辑，`Enter` 提交、`Esc` 取消 |
| 点纸条右侧锁按钮 | 定稿：锁住正文，完成与归档不受影响 |
| 点已完成纸条的归档按钮 / 顶部「归档已完成」 | 将已完成纸条从列表收起（记录保留在 `inbox.md`） |
| 顶部「已归档 N」 | 翻开归档抽屉，每条都能一键恢复 |
| `Ctrl+Z` | 回滚上一次正文修改、勾选或归档（仅本次运行内） |
| `Ctrl+F` | 就地过滤当前列表（`Esc` 退出），纯内存、即时生效 |
| 输入框为空时按 `↓` | 焦点进入列表；`↑`/`↓` 上下移动，首行再按 `↑` 回到输入框 |
| 顶部「导出」 | 导出一份可读 Markdown（不含 `<!-- slip:v1 -->` 内部标记） |
| 顶部「复制诊断」 | 把 vault 路径、热键、`content_rev`/`ui_rev`、健康与诊断复制到剪贴板，方便贴给别人排查 |
| 顶部「备份」 | 列出历史备份（显示条目数与首条摘要），两步确认后恢复；**恢复前也会先存一代** |
| 点正文里的 `http(s)` 链接 | 用系统默认浏览器打开（`file:`、`javascript:` 等一律拒绝） |
| 左键点托盘图标 | 唤出纸片 |
| 点窗口外任意位置 | 热键唤出的纸片自动收起（启动展示出来的窗口不会，它有别的语义） |

几个刻意的细节：

- **输入法选词时按 Enter 不会提交半截拼音。** 组合态被显式放过。
- **收录后焦点回到你刚才用的那个应用**，不会把你扔回桌面。
- **输入到一半的草稿只在内存里**：用热键或托盘收起窗口，草稿留着（回来就能接着写）；
  按 `Esc` 则视为放弃并清空。关掉应用草稿不落盘——`inbox.md` 是唯一真源，
  没提交的东西不该有个影子文件。
- 正文支持三种行内标记：`**粗体**`、`` `代码` ``、裸链接。渲染走结构化标签，
  **不注入 HTML**——`inbox.md` 是你随时能改的文件，它的内容不该被当作代码执行。
- 再次双击 exe 不会开出第二个实例，只会唤出已有的窗口。

托盘菜单：

```
热键：Ctrl+Alt+Space        ← 只是告诉你按什么，点不动
─────────────────────
显示 Slip
打开 Inbox
开机自启                    ← 可勾选，默认关闭
退出
```

---

## 数据在哪

默认 vault：

- Windows：`%USERPROFILE%\Documents\Slip\`
- Linux：`$HOME/Documents/Slip/`；没有 `Documents` 目录时用 `$HOME/Slip/`

用可见路径而不是隐藏的 appdata，因为
**Markdown 才是真源**——你随时可以用任何编辑器打开、手写、批量改。

`inbox.md`，每行一条：

```markdown
- [ ] 买发票 <!-- slip:v1;id=01a1c4a1-078c-718a-babc-d9bf25f8da03;created=2026-11-10T08:44:09.9965974Z -->
- [x] 交水电费 <!-- slip:v1;id=01a1c4a9-9fc2-7152-bcf8-b51092a43e53;created=2026-11-10T08:50:00.0Z;done_at=2026-11-10T09:00:00.1Z -->
- [ ] 定稿的那条 <!-- slip:v1;id=01a1c4b7-3d0e-7f21-9a44-0f1c2d3e4f50;created=2026-11-10T09:10:00.0Z;locked=1 -->
```

- `<!-- ... -->` 里是 Slip 的元数据（id 与时间）。**它是这条记录的身份**：
  删了它，这行就变成普通文本，不再出现在列表里（但也不会被删，原样留在文件中）。
- 方框和正文可以随便改，下次启动会读到。
- `locked=1` 表示这条已定稿，界面里正文不可再改；去掉这个键即解锁。
- 想手写新增一条，得带上 `<!-- slip:v1;id=<uuid>;created=<RFC3339> -->`。
  这是当前版本的已知不便，见下方限制。

偏好（当前只有热键）存在同目录的 `ui-state.json`。**删了丢了不影响 `inbox.md`
里的任何内容**——但自定义的热键会随之丢失、回到默认的 `ctrl+alt+space`。文件损坏时
按同一语义处理：回默认值，不阻塞内容读取。

---

## 配置

### 换 vault 位置

```powershell
$env:SLIP_VAULT = "D:\Notes\Slip"
```

```bash
export SLIP_VAULT="$HOME/Notes/Slip"
```

### 换热键

编辑 `<vault>/ui-state.json`：

```json
{ "hotkey": "ctrl+alt+s" }
```

写法是 `ctrl` / `alt` / `shift` / `super` 用 `+` 连一个键名。写错了不会崩：
自动回落默认的 `ctrl+alt+space`，并在日志里留一条 warn。

### 看日志

```powershell
$env:SLIP_LOG = "debug"   # error | warn | info | debug | trace，默认 info
```

```bash
export SLIP_LOG=debug
```

日志写到 stderr。启动时 stdout 会打印 vault、偏好文件和实际生效的热键各一行。

---

## 已知限制

诚实列出来，这些是**取舍**不是待办：

- **Windows 11 是主平台，Linux 外壳也能编、能跑，但验证程度不同。** Windows 上做过真实界面与
  端到端验收；Linux 目前只覆盖编译、clippy、lib 单测与基础启动冒烟，**GUI/E2E 尚未覆盖**。
  Linux 首版以 X11 为准：Wayland 下全局热键可能不可用，托盘行为受 AppIndicator/桌面环境影响，
  透明窗口、置顶与圆角取决于合成器。Linux 上打开 Inbox/链接/附件还需要 `xdg-open`。
- 跨不同 DPI 缩放混用（例如 100% + 150%）尚未实测：开发机所有显示器的缩放都相同。
- **单条写入 p50 约 13 ms、p95 约 15 ms**（Windows、debug 构建，1000 条之上的连续追加），
  来源是「整文件重写 + 备份 + fsync」。这是"Markdown 为真源、不加索引"的代价：
  换来的任何提速都只能来自降低耐久性，而那会改变崩溃后的行为。
  启动解析随行数线性增长：1000 行约 14 ms、10000 行（约 1.1 MB）约 75 ms，
  仍在可接受范围，因此**不引入索引**。完整数据与复现命令见
  [`docs/perf-baseline.md`](docs/perf-baseline.md)。
- 热键只能改配置文件，没有图形界面。
- **手写新增条目必须带上 `<!-- slip:v1;id=...;created=... -->`**，否则那行只是普通
  Markdown 文本。读取和修改是自由的，只有"新增"这件事目前需要贴一段元数据。
- **撤销只在本次运行内有效，且覆盖正文修改、勾选与归档。** 撤销栈在内存里，关掉应用
  就清空——历史不落盘，因为 `inbox.md` 是唯一真源。追加无法撤销（没有删除变更），
  锁定有显式开关可以直接点回去。
- **锁定只锁正文。** 已定稿的纸条仍然可以勾选完成和归档，否则锁会阻止任务完成。
- **不提供安装包。** `tauri.conf.json` 的 `bundle.active` 为 `false`，只分发绿色
  `slip-shell.exe`（Win11 自带 WebView2 运行时，无需另装组件）。想要 MSI/NSIS
  安装包时把它改成 `true` 再走 tauri CLI 打包即可。
- 没有标签、没有提醒、没有项目分组、没有云同步。
  v1 只做「捕获」这一件事，其余一概不做。

---

## 开发

- 架构与模块边界：[`docs/design/Slip-架构与模块定义.md`](docs/design/Slip-架构与模块定义.md)
- 构建、测试门禁、编码约定：[`AGENTS.md`](AGENTS.md)
- 端到端验收（真实热键 + CDP + 磁盘取证，**仅 Windows**）：

```powershell
pwsh -File tools/verify-hotkey.ps1
```

- Linux 侧没有等价的端到端脚本；CI 的 `shell-linux` job 只验证编译、clippy 与 lib 单测。

提交前门禁装一次：`git config core.hooksPath .githooks`（本地配置，不随仓库走）。

## 许可证

[MIT](LICENSE)。
