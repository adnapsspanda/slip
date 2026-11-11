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
5. **门禁失败时，先问"这条断言依赖了什么没写出来的前提"，再去看被测代码。**
   本会话两次假红都是这个形状：一次是 `Tauri Window` 这个类名被 Clash Verge 共用，
   脚本抓到了别人的窗口；一次是显示器从双屏变单屏，翻转断言的前提（右下放不下）
   不再成立。两次的失败信息都指向了错误的地方。

## 待办

| ID | 事项 | 证据 | 验收标准 | 状态 |
|---|---|---|---|---|
| R-02 | 死视图路径（**需要产品决策，勿擅自删**） | 整个 `ui/` 零引用 `toggleCollapse` / `DragEnd` / `DockSideChanged` / `CollapseToggled`；后端仍保留 `ViewEvent` 四变体、`Command::ToggleCollapse`、`PaperViewSet.collapsed/dock_side`，以及**只为它们服务**的 `PREFS_DEBOUNCE`（仅 `dispatch.rs:400`、`:418` 触发） | 二选一并在 `docs/design` 落定：① 接线（需要先定义 UI 入口）；② 整体删除并把 `prefs` 收窄，同时保证旧 `ui-state.json` 仍可读（serde default）。删除可连带简化 `dispatch` 的去抖机制 | 待决策 |
| R-06 | `csp: null` | `tauri.conf.json` 未设 CSP。当前无远程内容、无 `{@html}`，风险低但无纵深 | 补最小 CSP（`default-src 'self'`），确认 Svelte 内联样式与 WebView2 下仍正常：验收全过 | ✅ 见完成记录 |
| R-07 | 前端无 lint/format | 只有 `vite build`；格式靠人。仓库其余部分都有门禁 | 引入 `prettier --check` 并接进钩子与 CI；**不**引入 eslint 规则集 | ✅ 见完成记录 |
| R-08 | `.ps1` 必须带 BOM | `edit`/`write` 工具会剥掉 BOM，导致 `verify-hotkey.ps1` 被按 GBK 解码而报语法错，本次会话撞了 4 次 | 钩子 + CI 双向拦截：缺失则拒绝提交/构建失败，并给出修法 | ✅ 见完成记录 |
| R-05 | 限制常量前后端各一份 | `slip-domain` 的 `MAX_TEXT_CODEPOINTS=2000` / `MAX_LINES=100` 与 `Capture.svelte` 的字面量重复，无单一来源 | 加一条会红的契约测试，把两处钉在一起 | ✅ 见完成记录 |
| R-09 | 无安装包 | `bundle.active: false`，只能分发绿色 exe | 确认是否刻意；若是，在 README「已知限制」里写明不提供安装包及原因 | 待办 |
| R-03 | 摆放兜底居中到主屏 | **降级**：原证据（用户两次启动坐标恒为 `(965,344)`）实际来自沙盒启动，交互桌面上 `place_near_cursor` 正常跟随光标 | 仅在能复现"真实桌面下光标不可读"时再修；修法是把兜底顺序改为 光标所在屏 → 前台窗口所在屏 → 主屏 | 降级 |
| R-01 | ~~归档撤销承诺与实现不符~~ | **撤回**：`Patch::Unarchive`（`model.rs:52`）、`apply_patch` 分支（`state.rs:135`）、`inverse_batch` 双向映射（`dispatch.rs:565-568`）、领域测试（`state.rs:304`）、端到端测试（`pipeline.rs:301` `archive_and_undo_preserve_done_state_through_disk`）全部就位 | 无需改动。文档承诺与实现**一致** | ✅ 撤回 |

## 完成记录

- **R26｜本轮**：备份文件管理器（列出 + **可逆恢复**）。
  - 先核实再动手：确认项目**真的产生备份**——每次提交留一代
    `inbox.md.bak-<ms>`，最多 5 代（`BACKUP_KEEP`）。
  - 恢复写在**存储层**（`Store::restore_backup`），走 `Command::RestoreBackup`：
    外壳的 IPC 不直接写文件，守住"单消费者拥有写权"这条不变量。
  - **恢复前先给当前内容留一代备份**，所以恢复本身可逆——实测恢复后备份区多了一份
    227 B 的就是它。这条不做的话，"恢复错了"就没有第二次机会。
  - 名字校验只在存储层：拒绝含 `/`、`\`、`..` 的名字，且必须能被
    `parse_backup_name` 认出（= 本目录下真实的备份），否则等于一个"用任意路径覆盖
    inbox"的接口。有测试钉住四种非法输入且 inbox 未被改动。
  - 恢复后走正常的 `reload()` 载入路径（解析、校验、清 `needs_reload` 都不重写），
    同时**清空撤销栈**：恢复是整份覆盖，旧撤销记录再重放会得到莫名其妙的状态。
  - UI 复用列表现有样式；恢复用**两步确认**（第一下变"确认？"），因为它是破坏性操作。
  - 3 条 fsops 单测 + 探针 3 条断言，测试 212 → **215**，端到端 75 → **78**。
  - 实测：`["原始内容","写进去的第二条"]` → 恢复 → `["原始内容"]`，磁盘同步回退。

- **R27｜本轮**：外部修改的差异提示。原来只说"文件被外部修改"，现在说**改了多少条**。
  - `slip_store::peek_slips` 是**纯只读**解析：不碰内存文档、不改 `content_rev`、不写盘。
    关键是它不复制载入路径——复用 `doc::parse` 与 `Document.records`，不引入第二条
    解析实现（那会立刻变成"两套口径"的隐患）。
  - `system::diff_summary` 用 `(id, text, done)` 三元组比较：只报**人会注意到的变化**，
    标记顺序、时间戳差异不算改动（有测试钉住"内容没变就报 0"）。
  - 前端在 `needsReload` 为真时取一次差异（不轮询），横幅显示
    `磁盘上的文件已被外部修改（新增 N、消失 M、改动 K），需要重新载入。`
  - 3 条单测（三类差异各算一次、完全相同报 0、完成态变化算改动）+ 探针 2 条断言，
    端到端 73 → **75**，测试 209 → **212**。
  - 实测：外部改 1 条 + 加 1 条 → 横幅「新增 1、改动 1」，同时我的写入被拦住没有落盘。
  - 过程中的两个小坑：① 探针里 `evaluate` 遇到页面异常会返回 `{__exception}`，
    断言只看字段就会显示 `undefined`，把异常文本带进 detail 才能定位；
    ② 探针缺 `node:fs`/`node:path` 导入时我的插入锚点没匹配上、**静默失败**——
    改成按"缺少才加"逐个补。

- **R30｜本轮**：存储压力回归，产出 `docs/perf-baseline.md`。
  - 4 项 `#[ignore]` 性能测试实跑全过，数字如下（debug 构建）：

    | 项 | 实测 | 门限 |
    |---|---|---|
    | 单条追加 p50 / p95 / max | **12.97 / 14.50 / 20.15 ms** | 30 / 50 / 500 ms |
    | 勾选 p50 / p95 / max | **12.84 / 13.91 / 18.26 ms** | 同上 |
    | 启动解析 1000 行 | **13.84 ms** | 1000 ms |
    | 启动解析 10000 行 | **75.33 ms** | 2000 ms |
    | 每记录字节（10000 条） | **110.0 B**（总 1074.2 KB） | 64–256 B |

  - 两条值得写进结论的观察：
    ① 追加与勾选几乎同样快（都是 13 ms 级）——成本在 `fsync`，不在序列化。
       想再快只能降低耐久性要求，那会改变崩溃后的行为，不属于性能优化；
    ② 启动解析斜率约 **6.9 µs/行**，10000 条仍在 100 ms 内，**不需要索引**——
       这也为"v1 不上 SQLite"提供了实测依据。
  - 文档里写清了门限为什么宽容（测量机常在被抢占，抖动可达一个数量级），
    避免后来者把"门限通过"误读成"性能达标"。

- **R24｜本轮**：统计概览。
  - 状态行只放**人会关心的数**：`未完成 · 已完成 · 已归档 · 今日 +N · 损坏文件`；
    `rev` 与 vault 路径移进 `title`——前者是排查用的，后者太长，都不该占常驻位置。
  - 今日新增按**本地日界**算，且**不轮询**：跨零点时会短暂偏大，下次快照更新即修正，
    为它挂定时器不值得（与 R19 同一判断）。
  - 统计口径保持"未归档侧"，归档抽屉里不会出现"3 条未完成"这种困惑。
  - 探针 2 条断言，端到端 71 → **73**。实测输出：
    `meta = 1 未完成 · 4 已完成 · 今日 +5`、`title = .tmp-vault · rev 19`。

- **R23｜本轮**：导出为可读 Markdown（剥掉机器标记）。
  - 内容取自**视图快照**而不是复制 `inbox.md`：后者带 `<!-- slip:v1 -->` 标记，
    导出的东西是给人看的，不该把人造标记一起带走。
  - `system::render_plain` 是纯函数（因此可单测），`write_export` 落
    `<vault>/export-<unix_ms>.md`，写完在资源管理器里 `/select,` 定位。
    **不动 `inbox.md`**，所以不做确认弹窗。
  - 2 条单测（状态标记正确、空清单说明），实测导出内容：
    ```
    # Slip 导出

    - [ ] 买发票
    - [x] 已完成的活
    ```
    没有任何 marker。探针 1 条断言，端到端 70 → **71**。

- **R21｜本轮**：已归档抽屉（可恢复）。规划只要求"浏览"，但**只能看不能动**的抽屉
  是个死胡同——归档在界面上是"眼不见为净"，`archived_at` 是墓碑不是删除，
  所以补了 `unarchive` 命令与"恢复"按钮。
  - 统计口径改到 `activeSlips`（未归档侧）：在归档抽屉里看到"N 条未完成"只会让人困惑。
  - 界面上新增 14 → **15 条 IPC 命令**（`unarchive`）。
  - 探针 4 条断言，端到端 66 → **70** 全过。其中两条一开始是红的：
    ① 我的正则写成 `/^已归档 \\d+$/`（匹配字面反斜杠）——**断言写错**；
    ② **真 bug**：恢复掉最后一条后仍停在归档视图，用户对着一张"还没有归档"的空表。
       修法：恢复成功后若归档已空则自动退出抽屉。这条是探针替我发现的。

- **R19｜本轮**：纸条上的相对时间。
  - `ui/src/lib/time.js` 里两个**纯函数**，`now` 可注入（否则边界测不了）：
    `relativeTime(iso, now)` 与 `exactLocal(iso)`。无法解析的时间返回空串——
    界面据此不显示，而不是显示一个 `NaN`。
  - **不用定时器轮询**：纸条上的"10 分钟前"不需要每秒准确，为它挂 setInterval 会让
    整个列表每秒重渲染。渲染时算一次，快照更新或重新挂载时自然再算。
  - 边界处理：时钟回拨（`seconds < 0`）归为"刚刚"，不会显示负数；跨年显示年份。
  - `<time datetime>` 带 RFC3339，`title` 给精确本地时间——需要准确值时看那里。
  - CDP 实测五档（用真实时间戳播种）：`刚刚` / `5 分钟前` / `3 小时前` / `昨天` /
    `11 月 1 日`；`title` 是 `2026/11/11 13:36:49`（= `05:36:49Z` + 8h），
    `datetime` 属性保留原文。

- **守卫补强（随 R17）**：`tools/verify-hotkey.ps1` 的时序守卫原先只比 `ui/dist` 与 exe，
  现在**也比 Rust 源码与 exe**。R17 我就是靠这条错误白查了一轮：改了 `slip-domain`
  却只跑 `cargo test -p slip-domain`，验收跑的是旧二进制，结果是"改动没生效"。
  负向验证：把某个 `.rs` 的时间戳摸新 3 分钟 → `[STALE] Rust 源码比 slip-shell.exe 新`
  并以退出码 2 拦住；重编后恢复。
  **环境恢复**：本次复跑端到端 **59 PASS / 0 FAIL / 1 SKIP**，之前 R15/R16 期间的
  4 项红确认为沙盒导致（当时已用"回退到上一提交同样红"与"explorer 启动正常"双重证明）。

- **R17｜本轮**：粘贴 Markdown 清单时剥掉列表前缀。
  - 实现在 `slip-domain/split.rs::strip_list_prefix`，三条刻意的保守规则：
    ① 分隔符后**必须跟空格**（`1.5 倍`、`-5 度`、`-- 注意` 都不是前缀）；
    ② 只剥**一层**（`- - 嵌套` → `- 嵌套`）；
    ③ 有序列表数字最多 3 位（`2026.11.11` 不会被当序号）。
    checkbox 只在列表标记**之后**才剥——裸的 `[ ]` 更像正文里的方括号。
  - `- ` 这种空列表项按空行丢弃；剥完只剩空白的捕获等价于空捕获。
  - 5 条单测覆盖：四种常见前缀、checkbox 位置、五类"长得像但不是"的输入、
    只剥一层、空列表项、剥完后的长度上限（`- ` 不占配额）。
  - **真实链路验证**（CDP + 读盘）：粘贴 8 行得到 7 条，
    `- 买发票`→`买发票`、`2) 第四件`→`第四件`、`- [ ] 第五件`→`第五件`，
    而 `1.5 倍不是前缀` 与 `-- 注意也不是` 原样保留，末尾 `- ` 被丢弃。
  - **过程教训**：第一次验证得到"前缀没剥"的结果，原因是我改了 `slip-domain`
    却只跑了 `cargo test -p slip-domain`、**没重编 shell**，验的是旧二进制。
    与之前 `ui/dist` 比 exe 新是同一类错误——见下一行的守卫补强。

- **R15｜本轮**：行内渲染（粗体 / 行内代码 / 裸链接），**不引入 HTML 注入面**。
  - 手工切分（`ui/src/lib/inline.js`）而不是 `{@html}`：正文来自 `inbox.md`，
    这个文件用户随时可编辑，当 HTML 插进 DOM 就等于给它一个执行入口。
    渲染时每一段要么是纯文本（Svelte 自动转义），要么是 `<strong>`/`<code>`/`<a>`
    三种已知标签。
  - 链接末尾的标点（`。`、`）`、`，`…）从 href 里剥掉并还给正文——否则
    「见 https://a.com。」的句号会消失，链接也会 404。
  - 打开链接走新命令 `open_url`，**校验全在 Rust 侧**（`system::validate_url`）：
    只认 http/https，拒绝 `file:`/`javascript:`/`ms-settings:` 等前缀与含空白、
    控制字符的输入；执行用 `rundll32 url.dll,FileProtocolHandler`，URL 作为单个
    argv 传递，**不经过 `cmd /c start`**（后者会把 `&` 当命令分隔符，那是注入）。
  - 校验与执行拆开（`validate_url` / `open_url`），所以能单测而不真的弹浏览器：
    3 条测试覆盖合法前缀、7 种非法前缀（含 `file:`、`javascript:`、空 host）、
    空白与控制字符。
  - CDP 实测（6 项全过）：`**重点**` → `<strong>`、`` `npm run build` `` → `<code>`、
    href 去尾标点而正文保留 `。`、`open_url` 接受合法链接且**拒绝 `file:`**。
  - **说明**：本轮未能把这几条并入 `tools/verify-hotkey.ps1`——该套件在当前
    沙盒环境里跑不出可见窗口（见 R16 记录）。不想留下一批没跑过的断言，
    待环境恢复后再并入。

- **R16 + R18｜本轮**（合并做，因为不可分）：删除前端零入口的视图状态路径。
  `ViewEvent` 与三个命令变体一删，`Prefs` 里那些字段与 `dispatch` 的去抖机制就都成了
  孤儿——分开提交只会在中间态留下死代码。

  删除清单：`MonitorSnapshot` / `MonitorGeometry` / `ViewEvent`（四个变体）/
  `Command::ToggleCollapse` / `Command::ViewEvent` / `Command::FlushPrefs` /
  `PaperViewSet` 的 `order`/`collapsed`/`dock_side`/`monitors` 与 `ordered`/`drag`/
  `toggle_collapsed` 方法 / `Presenter::apply_view_event` 等五个方法 /
  `PREFS_DEBOUNCE` + `prefs_dirty` + `prefs_due` + `persist_prefs` +
  `view_event_dirties_prefs` / `Prefs` 的 `order`/`collapsed`/`dock_side` / `DockSide` /
  `flush_prefs` IPC 与 `bridge.flushPrefs`。

  **净变化：6 个文件，+50 / −522 行。** 测试 204 → 198（少掉的是只为死路径存在的用例）。
  兼容性：旧 `ui-state.json` 里的 `order`/`collapsed`/`dockSide` 会被 serde 忽略，
  老文件照常能读——`prefs_written_before_hotkey_existed_still_load` 覆盖了同类场景。

  **本轮验收的诚实说明**：`cargo test` 198 项、fmt、clippy 全绿；但 **59 项端到端
  在本次环境里无法通过**——原因是启动它的终端有沙箱，窗口 `WS_VISIBLE` 恒为假。
  已用两种方式证明这与本次改动无关：① `git stash` 回到上一提交重编，同一条断言
  一模一样的失败；② 由 `explorer.exe` 启动同一个 exe，窗口 `vis=True`。
  该症状已补进 `AGENTS.md` 的验收先决条件（"窗口存在但不显示"）。

- **R14｜本轮**：本地即时过滤（Ctrl+F）。纯内存，不引入任何索引。
  - 设计：`filter` 只影响显示。撤销栈按**后端真实提交顺序**记录，过滤若参与排序或
    索引，`Ctrl+Z` 就会撤错东西——所以过滤严格留在视图层。
  - "**过滤没匹配到**"和"**真的没有内容**"分成两种空态：给过滤空结果看首次使用引导，
    用户会以为数据没了，比没有空态更糟。
  - 过滤状态下勾选正常（点击走的是真实 slip，与过滤无关）。
  - 实测性能（1000 条、125.8 KB）：

    | 指标 | 结果 |
    |---|---|
    | 纯计算（100 次平均） | **0.064 ms** |
    | 端到端 输入→下一帧（1 行） | 8.2 ms |
    | 端到端 输入→下一帧（272 行） | 12.9 ms |
    | 端到端 输入→下一帧（0 行） | 2.6 ms |

    门禁要求「< 10ms」在**计算**层成立（0.064ms）；端到端两个数包含等待下一帧
    （60Hz 下 rAF 本身可达 16ms）与 DOM 重建，不是过滤算法的成本。按实测如实记录，
    不把 12.9ms 说成"达标"。
  - 探针补 5 条，端到端 54 → **59**。
  - 顺带修掉我自己写错的一条断言：`filtered.error === undefined` 永远是 false，
    因为 JSON 里显式是 `null`；改成 `!filtered.error` 后通过（功能本来是好的，
    是断言写错了）。

- **R12｜本轮**：切换编辑目标不再丢改动。**先实测**，测出一个双向数据丢失 bug。
  - 实测（编辑 A 改内容 → 不按 Enter 直接双击 B）：A 的改动**没保存**、B 也**没进入
    编辑**。根因是 `commitEdit` 读的是**可变的** `editingId` / `draft`——切换时这两个
    值已被改写成 B 的，提交逻辑就对着"新那一条"做判断，`draft === slip.text` 成立
    于是静默返回；旧输入框卸载时的 blur 又用 B 的会话把新编辑框关掉。
  - 修法：引入不可变的 `session = { id, original }`，提交针对"这一次编辑"；
    `handleBlur(slipId)` 校验 `session.id === slipId`，把卸载式失焦挡在外面；
    `finishSession(id)` 只在这条仍是当前编辑目标时才清界面状态；`beginEdit` 切目标前
    先按上一条自己的会话提交。
  - Ctrl+Enter 本来就通（`event.key === "Enter"` 与修饰键无关），探针里补了断言钉住。
  - 探针补 3 条断言，端到端 51 → **54**，全过；磁盘上两条改动都正确落盘。
  - 顺带记一笔方法论教训：我第一版实验脚本按索引取 `.slip__text`，而编辑开始后那一行的
    span 会被换成 input，索引随即错位——**测试脚本本身也会踩"假设的 DOM 形状"**。

- **R11｜本轮**：全键盘流。`录入 → ↓ 进列表 → Space 勾选 → Enter 编辑 → Esc 退出 → ↑ 回捕获框`，
  全程不碰鼠标。
  - 不新增"当前选中项"状态：**DOM 焦点就是它**。两份状态就要回答"焦点和选中
    不一致时听谁的"，而那没有正确答案。
  - Capture：内容为空时按 ↓ 才劫持（有内容时用户多半想移动光标）；交给 App 调
    `listRef.focusFirst()`。
  - SlipList：`bind:this` 收集各行正文元素，`handleItemKeydown` 处理 ↑↓/Space/Enter；
    空格必须 `preventDefault`，否则勾选的同时页面会滚动。
  - 焦点交还：键盘进入编辑就用键盘交还（`refocusRow` 在 commit/cancel 后 `tick()` 再
    聚焦那一行）；**失焦提交不走这条路**，否则会把用户刚点去的输入框抢回来。
  - 探针补 5 条断言，端到端 46 → **51**，全过。磁盘上确认出现了用 Space 勾选的那条
    `- [x] 键盘流测试的一条`（含 `done_at`）。

- **R10｜本轮**：阶段一验收。产出 `docs/phase-1-report.md`，数据全部来自执行：
  - 门禁：前端构建/格式、`*.ps1` 编码与语法、fmt、clippy、**204 项测试**、
    **46 项端到端 / 0 失败 / 1 跳过**；
  - 架构逐条核实：依赖单向（`slip-app` 无 tauri 依赖）、domain 零 I/O、
    **全仓库无 `#[allow(...)]` 压制**、无 `dbg!`/`TODO`/`console.*`、无未跟踪文件、
    14 条 IPC 命令全部有业务含义、权限仅 `core:default`、CSP 生效。
  - 报告里单列了「明确未验证」四条（混合 DPI、负坐标副屏端到端、视觉、性能测试
    默认不跑）与「未做及原因」，避免把跳过写成通过。
  - 清理检查：`println!` 仅剩启动三行与 perf 报表（均刻意）；`doc.rs` 里两处
    "todo" 是注释里的普通单词，不是 TODO 标记。

- **R07｜本轮**：外部占用。**先实测再改**，结果和规划里的假设不一样。
  实测（真实持有独占句柄 + CDP 触发收录）：
  - 占用期间收录失败、草稿保留（这部分原本就是对的）；
  - 但诊断只写 `Other`——Rust 把 Windows 的共享冲突归进 `Uncategorized`；
  - **释放句柄后仍然拒绝写入**：一次瞬时占用让 vault 永久降级为只读，
    只能靠用户手动点「重新载入」恢复（实测 reload 确实能恢复）；
  - 横幅说「文件无法解析」，与诊断里的"读取失败"直接矛盾。

  三处修复：
  1. `IoKind::Busy`：按原生码区分 `ERROR_SHARING_VIOLATION(32)` /
     `ERROR_LOCK_VIOLATION(33)`；并给 `IoKind` 实现 `Display`，诊断从
     `Other` 变成「文件被其它程序占用」。
  2. `read_for_conflict`：写入前的读盘对 `Busy` 短重试（5 次 × 40ms）。
     **只对 Busy 重试**——权限不足、文件不存在重试多少次都一样，只会拖慢报错。
  3. 前端横幅/`blockedReason` 改成原因中立（「当前不可写入，详见横幅诊断」），
     具体原因交给 diagnostics。

  测试 4 条，其中两条是"证明重试有用"而非"有重试"：
  - `sharing_violation_maps_to_busy`：原生码 32/33 → Busy，且 5(ACCESS_DENIED)
    不被误判；
  - `exclusive_handle_reads_as_busy`：真实 `share_mode(0)` 独占句柄 → 读到 Busy；
  - `retry_rides_out_a_short_lock`：占用 60ms、重试预算 160ms → 读成功。
    把 ATTEMPTS 改成 1 或 DELAY 改成 1ms 会立刻变红；
  - `io_kind_renders_human_readable_text`：这些文字会显示在界面上，不是日志。

  实测确认：长占用时诊断显示「文件被其它程序占用」，提示区显示「当前不可写入，
  详见横幅诊断」，草稿保留。测试 200 → 204；端到端 46 项全过。

- **R04｜本轮**：跨屏定位先补测试再谈改算法。
  `place_near_cursor` 依赖 `AppHandle`（读光标、读显示器、读窗口尺寸），本身测不了，
  所以先把**纯几何**抽成 `placement(cursor, area, size, gap) -> (x, y)` 与 `Area` 结构，
  规则一字未改（先右下 → 放不下翻左上 → 最后夹紧进工作区，夹紧用 `.max()` 兜住
  "窗口比工作区还大"的退化情形，因为 `clamp` 在 `min > max` 时会 panic）。

  7 条单元测试，其中两条是端到端覆盖不到的：
  - `handles_monitor_with_negative_origin`：副屏整体在负坐标区
    （left=-454 / top=-1440 / right=2106 / bottom=-48）。端到端脚本在单屏机器上
    只能 SKIP 这条路径，自显示器变更后它就没被验证过。
  - `clamps_when_window_is_larger_than_work_area`：窗口大于工作区时必须保住左上角
    可见且**不 panic**。

  另有贴边判据（严格大于才算溢出：`cursor+gap+width == right` 不翻）、
  两个翻转方向，以及"光标在任意角落结果都在工作区内"的横扫。

  验证：7 项单测通过；全量 **200 项**（193 → 200）；端到端 46 项全过，两个场景仍是
  `adj=(0,0)`——重构没有改变运行时行为。顺带观察到本次运行时分辨率变回 1707×1019，
  而屏幕推导的坐标自动跟上了。

- **R09｜本轮**：损坏文件角标。
  - 后端：`system::corrupt_artifacts(vault)` 扫 `inbox.md.corrupt-*`（**只认这个前缀**，
    vault 是用户可写目录，扫出别的文件当"损坏"会误导人），按 mtime 倒序；
    `reveal_newest_corrupt` 用 `explorer.exe /select,<path>` 选中最新那个。
  - 命令面只增两条，且都刻意收窄：`corrupt_files` **只返回文件名不返回绝对路径**；
    `reveal_corrupt` **不接收任何参数**——定位目标由 Rust 自己从 vault 查，
    否则等于让 webview 决定 explorer 打开什么。
  - 前端：`meta` 行显示"N 个损坏文件"，仅在确实存在时出现一个`定位`按钮，
    `title` 里列出文件名供核对；读取失败当作没有（它是提示不是门禁）。
  - 验证：造两个损坏证据 + 一个干扰文件 `my-notes.md` 的 vault，
    实测 `meta = "r09-vault · 0 条未完成 · 2 个损坏文件 · rev 1"`，
    `title` 里两个文件按新到旧排列，干扰文件未被计入。
  - 探针补 2 条：`corrupt_files` 必须返回数组（否则 `corrupt.length` 会在界面炸成
    TypeError）、干净 vault 不显示角标。端到端 44 → 46。

- **R05｜本轮**：用算出来的数字改配色，不靠肉眼。
  实测发现两个可量化问题：
  1. `--ink-soft`（#8a8172）在纸色上只有 **3.59:1**，低于 WCAG AA 对正文的 4.5:1，
     而它承载的是时间戳、已完成正文、空态说明这些需要读清楚的小字 → 压暗到
     `#787061`（**4.58:1**），色相不变。
  2. 纸色 #fbf7ee 在**纯白壁纸**上只有 1.07:1，原边框 1.40:1，边缘近乎不可见；
     而深色壁纸上有 15.6:1、灰色壁纸 3.7:1，本就达标 → 补一圈深色发丝边
     `0 0 0 1px rgb(43 38 32 / 35%)`，白底合成后约 **2.1:1**。

  这里有一个**我没有假装达标**的地方：要满足 WCAG 1.4.11 的 3:1，发丝边得用 alpha 0.5，
  那会在奶油色纸边压出一圈明显灰框，把便签变成"带框的卡片"。而任意壁纸本就无法保证
  边界对比度，所以取 0.35 并把数字写进注释。**视觉最终仍需人眼确认**——我只能保证
  数字，保证不了观感。

  门禁：fmt / clippy / 193 测试 / 44 项端到端全过。

- **新维度：权限最小化审计｜本轮**。Tauri 的 capabilities 只约束 **webview → core
  的 IPC 调用**，Rust 侧直接调插件 API 不需要权限。按这个前提核实前端：
  `ui/src` 里只有两处 `__TAURI__`——`core.invoke`（`bridge.js:36`）与 `event.listen`
  （`bridge.js:105`）。`startDragging`、`window.hide/show/setFocus`、`globalShortcut`、
  `autostart` **一次都没被调用**（后两个本来就是 Rust 侧直接用插件）。

  于是把 11 项权限收成 1 项：`["core:default"]`。删掉的是
  `core:window:allow-start-dragging / allow-hide / allow-show / allow-set-focus`、
  冗余的 `core:event:default`，以及 `global-shortcut:*`、`autostart:*` 共 10 条。

  验证：重建后跑验收 **41 PASS / 0 FAIL**，其中「唤出后捕获框获得焦点」证明
  `event.listen` 仍通（`core:default` 已包含 event），「Tauri 桥可用」「回车后提示为
  已收录」「收录成功后窗口自动收起」证明 IPC 与自定义命令未受影响。
  约定写进 `AGENTS.md` 新增的「权限最小化」一节。

- **新维度：CI 可执行性验证｜本轮**。CI 因为还没有远端，**从未真正执行过**。
  做法：`git clone` 一份干净副本（无 `ui/dist`、无 `target`），按 workflow 顺序逐步跑。
  结果全部通过：

  | step | 结果 |
  |---|---|
  | 2 编码检查 | OK |
  | 4 `bun install --frozen-lockfile` + build | OK —— 说明提交的 `bun.lock` 与 `package.json` 一致（刚加过依赖，这条最容易红） |
  | 5 `prettier --check` | OK |
  | 7 `cargo fmt` | OK |
  | 8 clippy `-D warnings` | OK（冷编译 56s） |
  | 9 `cargo test` | OK（188 项，50s） |
  | 10 release 构建 | OK（73s，产物 10.2 MB） |

  冷启动总耗时约 3 分钟，远低于 `timeout-minutes: 30`。

  **顺带发现覆盖缺口**：CI 只检查 `*.ps1` 的 BOM，**不检查语法**。验收脚本被频繁
  手改（引号、here-string、中文注释都踩过），语法错在 CI 里完全看不见。
  新增 `tools/check-ps1-syntax.ps1`（解析全部 `*.ps1`，报出首个错误与行号），
  由提交钩子与 CI 共用同一份实现。正反验证：当前仓库「全部 .ps1 语法通过」；
  造一个缺右大括号的脚本 → 正确报出「第 1 行」且退出码 1。
  钩子 6 步 → 7 步，CI 10 步 → 11 步。

- **新维度：错误路径审计｜本轮**。核对三件事：
  1. 设计文档 §6.5 的 `StoreError` → `Ack` 映射表与 `map_store_error` /
     `result_for_code` **逐条一致**（conflict / needsReload / readOnly / domain /
     blocking / io / notDegraded 全对得上）。
  2. 前端横幅优先级与设计一致（Fatal > Isolated > Degraded > needsReload），
     `blockedReason` 覆盖 bridge 不可用、fatal、isolated、只读、needsReload 五种状态，
     9 处 ACK 处理**全部**走 `formatAckError`，没有绕过翻译的地方。
  3. **发现真缺陷**：`formatAckError` 缺 `blocking` / `io` / `notDegraded` 三个后端
     确实会返回的码，落到 `default` 把原始消息直接给用户；而 `blocking` 的消息是
     `format!("{err:?}")`——用户会在提示区看到
     `DuplicateId { id: SlipId(SlipId(019a…)) }` 这样的 Rust 结构体。

  修复：给 `BlockingError` 实现 `Display`（错误类型应当能自我描述，不该逼调用方
  Debug 它）；`map_store_error` 改用 `err.to_string()`；前端补三个 case，并遵守既有
  约定——后端消息保持英文（`domain` 分支靠 `msg.includes("locked")` 判断，改语言会
  让判断失效），翻译只在前端按 code 做。
  加守卫测试 `blocking_message_is_human_readable`：消息里出现 `{` 或 `SlipId` 就红。
  测试 187 → 188；端到端 41 项全过。

- **新维度：文档一致性审计｜本轮**。逐条把 `README` / `AGENTS.md` 里可验证的承诺
  对照代码核实（现场读代码 + grep，不凭记忆）。结果：

  | 承诺 | 结论 |
  |---|---|
  | 热键写错回落默认并留 warn | ✅ `hotkey.rs::resolve_shortcut` 确有 `tracing::warn!` |
  | 归档有单条按钮 + 顶部「归档已完成」 | ✅ `SlipList.svelte:184`、`App.svelte:436/439` |
  | 启动时 stdout 打印 vault / 偏好 / 热键 | ✅ 三行 `println!` 都在 |
  | 「`ui-state.json` 只是展示偏好」 | ❌ 它含 `hotkey`，`prefs.rs` 自己的注释就写着「它严格说不是展示偏好」。已改：**删了不影响内容，但自定义热键会丢失、回到默认** |
  | 「开发机两台显示器都是 150%」 | ❌ 现在只有一块屏。已改为「开发机所有显示器缩放都相同」，与台数解耦 |
  | AGENTS「覆盖 7 类语义」 | ❌ 实际远超 7 类。已重写为完整清单，并注明**条目数不要写死** |
  | AGENTS 钩子覆盖「fmt / clippy / test」 | ❌ 已含编码检查与 prettier。已更新 |
  | `PaperViewSet` 必须原样携带 `hotkey` | ✅ 有回归测试 `view_roundtrip_never_drops_the_hotkey` |

  顺带把「断言不得依赖未声明的环境前提」写进 `AGENTS.md` 的验收章节——
  本会话两次假红都是这个形状（类名撞车、坐标写死）。

- **R-07｜本轮**：接入 prettier 3.9.10 + prettier-plugin-svelte 4.1.1。
  - `ui/.prettierrc.json` 里显式声明 `plugins`。**这是必须的**：prettier 3 不会自动
    加载插件，缺了它 `.svelte` 会被静默跳过——`--check` 报"全部合规"，其实一个都没查。
    我第一轮测量时就上过这个当，误以为只有 2 个文件需要改。
  - `ui/package.json` 加 `format` / `format:check` 两个脚本；钩子与 CI 各接一条。
  - 真实改动面：5 个文件、+20/−11，全是空行与折行规范化，没有重排既有逻辑。
  - 负向验证：往 `bridge.js` 追加一行 `export const  bad = (a,b)=>{return a+b}`
    → `format:check` 退出码 1 并指出文件名；还原后恢复 `All matched files use
    Prettier code style!`。
  - 端到端：41 PASS / 0 FAIL，格式化没有改变行为。
- **R-09｜本轮**：README「已知限制」补一条——`bundle.active` 为 `false`，只分发
  绿色 exe（Win11 自带 WebView2），并给出想要安装包时的改动点。

- **R-05｜本轮**：新增 `crates/slip-domain/tests/frontend_contract.rs`，直接读
  `ui/src/lib/Capture.svelte` 源码，把 `MAX_CODEPOINTS` / `MAX_LINES` 两个字面量与
  `slip_domain::MAX_TEXT_CODEPOINTS` / `slip_domain::MAX_LINES` 断言相等。
  没有选「加注释互指」那种软办法——注释不会红，测试会。
  `declared_number` 返回 `Option`，所以「常量被改名/删掉」（None）与「数值变了」
  （Some(99) vs Some(100)）给出可区分的失败信息。
  双向验证：正常状态通过；把前端改成 `MAX_LINES = 99` 后
  `assertion left == right failed: Capture.svelte 的 MAX_LINES 必须等于 slip_domain::MAX_LINES`
  → 退出码 101；随后还原。
  测试总数 186 → 187。

- **R-08｜本轮**：编码守卫同时落在钩子与 CI。
  - `.githooks/pre-commit`：按**暂存区内容**（`git show ":$f"`）校验 `*.ps1` 的前三个
    字节是否为 `EF BB BF`，检查排在所有 cargo 步骤之前（几乎不耗时，且是唯一能用
    肉眼之外的证据发现"文件编码坏了"的地方）。用 `while IFS= read -r` + 临时文件
    收集失败项，路径含空格也不会漏。
  - `.github/workflows/ci.yml`：同一条检查作为第 2 个 step，理由写在注释里——
    钩子能被 `--no-verify` 绕过，CI 不能。
  - 双向验证：造一个无 BOM 的 `tools/_bom_probe.ps1` 暂存后跑 `git hook run pre-commit`
    → 退出码 1 并列出文件名与修法；补上 BOM 后重跑 → `[pre-commit] 通过`。
    探针已清理。CI 的检查逻辑在本机全仓库跑过一遍：`全部 .ps1 均带 BOM`。

- **R-06｜本轮**：`tauri.conf.json` 补 CSP：
  `default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:;
  font-src 'self'; connect-src 'self' ipc: http://ipc.localhost; object-src 'none';
  base-uri 'self'; form-action 'none'; frame-ancestors 'none'`。
  前置确认：`ui/src` 无内联 `style=`/`style:`、`ui/dist/index.html` 无内联 script、
  产物 JS 不动态插入 `<style>`，所以不需要 `unsafe-inline`。
  `connect-src` 必须显式放行 `ipc:` 与 `http://ipc.localhost`（Tauri v2 的 IPC 走这里）。
  验证：验收里「Tauri 桥可用」「回车后提示为已收录」「输入框已清空」全过，
  说明 `script-src 'self'` 与 IPC 都在 CSP 下正常工作。
- **顺带修掉验收脚本的一个真缺陷**：翻转场景把光标坐标写死成 `(1400,900)`。显示器从
  「2560x1600 + 3840x2160 双屏」变成「单块 2560x1440」后，该坐标水平方向其实放得下，
  「必须翻到左侧」的断言随即假红。改为由 `PrimaryScreen.Bounds` 推导
  （`$NearX/$NearY`、`$FarX/$FarY`），并把翻转判据补成左右 + 上下两个轴。
  副作用：`[SKIP] 负坐标副屏：本机只有一块显示器` 属预期，脚本本就该在单屏时跳过。
- **R-04｜4469d04 之后的文档提交**：设计文档 §9.7 补修订表第 14 行与 §9.7.2，
  说明 `spike_*` 命令集已删、自检迁到 `diag.rs`（`SLIP_LOG=debug`），并把
  §9.7.1 人眼清单第 3 条里对 `spike_hide` 的依赖改成 `Esc`/点窗口外。
- **R-01｜撤回**：上一轮把它当 P0 报出来，实际代码完整实现了归档撤销且有端到端
  覆盖。教训写进铁律第 4 条。
