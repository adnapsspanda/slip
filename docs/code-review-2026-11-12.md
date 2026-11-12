# Slip 代码审查与优化方案

- 日期：2026-11-12
- 范围：`crates/slip-domain`、`crates/slip-store`、`crates/slip-app`、`src-tauri`、`ui/src`、`tools/`、CI
- 方法：6 个子代理分模块只读审查，主审查逐条回读源码核验；逻辑层测试实跑通过（10 个目标 / 286 项 / 0 失败）
- 标注：**已核验** = 本次直接读过对应源码；**待复现** = 来自子代理推演，尚未构造触发用例

## 结论摘要

按「影响 × 触发概率」排序，最该先修的五件事：

1. **三条门禁守卫是假绿**：`check-forbidden.ps1` 的 `Include` 归一 bug 让 watcher / SQLite / CSP 永远通过；`check-ui-logic.mjs` 的退出判定写在文件中段，最后 14 条断言不参与退出码；`mutation-audit.py` 仍指向旧机器路径，整体跑不起来。
2. **不可逆删除没有真正的前置备份**：`purge_archived` 在备份失败时仍继续删行，违反设计 §175 的「先写备份」。
3. **最需要恢复时恢复不了**：`restore_backup` 先备份当前内容，而 `backup()` 对缺失的 `inbox.md` 返回 `NotFound`，恢复流程提前退出。
4. **撤销会静默失真**：`SetDone` 的逆操作漏掉「已完成但无 `done_at`」与「归档态」；`RestoreBackup` / `IsolateAndReset` 失败时也无条件清空撤销栈。
5. **运行期换热键不落盘**：托盘切到 `Win+Alt+S` 只改内存，重启回到未注册状态；前端横幅还继续显示旧状态。

---

## 一、门禁假绿（P0）

共同特征：退出码为 0，但约束根本没被检查。后续所有「测试通过」都建立在这些守卫之上。

### 1.1 `Include` 归一错误，三条不可回退约束永远通过

**位置**：`tools/check-forbidden.ps1:119-122`，调用点 `:166`、`:207`。

`$exts.Add(($glob -replace '^\*', ''))` 把 `'Cargo.toml'` 原样放进集合，而匹配用的是
`[IO.Path]::GetExtension($file)`（返回 `'.toml'`）。因此 `@('Cargo.toml')` 与
`@('tauri.conf.json')` 永远匹配不到任何文件：watcher、SQLite、CSP 三条检查恒为 `[通过]`。

**触发场景**：往根 `Cargo.toml` 加 `notify` / `rusqlite`，或把 `tauri.conf.json` 的
`csp` 改成 `null` / 加 `unsafe-inline`，门禁仍绿。

**建议**：`Include` 明确支持「扩展名」与「文件名」两种语义；`-SelfTest` 增加
「临时 `Cargo.toml` 写 notify 必须被抓到」的熔断用例（当前 SelfTest 只测正则，不经过 `Include`）。

**核验**：已核验。

### 1.2 前端纯逻辑的退出码写在中段

**位置**：`tools/check-ui-logic.mjs:559-561`；其后的断言在 `:570-609`。

`process.exit(1)` 之后还有列表边界 4 条 + 对比度/颜色 10 条，失败只打印 `[FAIL]`，不改变退出码。

**触发场景**：改 `listNavTarget` 边界或 CSS 颜色导致对比度不足，门禁照样绿。

**建议**：汇总退出移到文件最后一行；加一条结构守卫「最后一条 `check(` 必须在退出判定之前」。

**核验**：已核验。

### 1.3 `mutation-audit.py` 已整体失效

**位置**：`tools/mutation-audit.py:22-23`、`:155`、`:174`。

`REPO = r"D:\Dev\simple-todo"`、WSL 挂载点 `/mnt/d/Dev/simple-todo`、发行版 `Arch` 全硬编码，
在当前仓库会直接 `FileNotFoundError`。另外 `red = (code != 0) or (marker in out)` 把
**变异导致编译失败**也当成「测试变红」，语法写坏同样 PASS。

**建议**：路径由 `Path(__file__).resolve().parents[1]` 推导；「变红」判据收紧为输出含
`test result: FAILED` 且不含 `error[E` / `could not compile`。

**核验**：已核验。

### 1.4 门禁对等性只在「已知标签」层成立

**位置**：`tools/check-ci-parity.ps1:48-65`、`:106`。

- 无法命中关键词的步骤被 `Where-Object { $_ }` 静默丢弃，新增 CI 安全审计步骤不会被发现；
- 比对对象是 `step "..."` / `- name:` 标签，不是实际命令。删掉 `cargo fmt --all --check` 但保留标签，仍然绿；
- `:106` 只检查「钩子 workspace、CI 不是」这一个方向。

**建议**：未识别的步骤默认 `[FAIL]`（白名单显式放行）；每个关口同时断言标签与命令片段。

**核验**：已核验（结构）；具体漏报项待复现。

### 1.5 注释屏蔽不识别字符串，可被一行 URL 绕过

**位置**：`tools/check-forbidden.ps1:56-59`。

`Mask-Comments` 逐行处理，遇到 `//` 就抹掉本行剩余部分，不区分字符串上下文。形如
`const base = "http://127.0.0.1"; await fetch(base);` 的真实违规会被当成注释隐藏。
同类风险：字符串里的未闭合 `/*` 会吞掉后续行直到下一个 `*/`。

**建议**：先识别 `'` / `"` / `` ` `` 字符串态再判注释；SelfTest 增加「含 `http://` 的同业违规仍被抓到」
与「跨行块注释不误报」两组样本。同时把 `-SelfTest` 接入钩子或 CI（目前未接入）。

**核验**：已核验（逻辑）。

---

## 二、数据安全与恢复（P0/P1）

### 2.1 `purge_archived` 备份失败仍继续不可逆删除（P0）

**位置**：`crates/slip-store/src/lib.rs:610-627`。

`fsops::backup` 失败只把 `Warning::BackupFailed` 写进 health，随后照常 `write_atomic` 删行。
设计文档 `docs/design/Slip-架构与模块定义.md:175` 明确「彻底移除」有三道保护：只允许删有效归档、
**先写备份**、其余行逐字节保留。§459「备份失败不阻断主文件写入」是常规写入的规则，不适用于
唯一不可逆、且不提供撤销的操作。

**触发场景**：磁盘满、`PermissionDenied`、文件被占用时点「彻底删除」。用户以为有备份，实际没有。

**建议**：`purge_archived` 在 `backup` 或 `prune` 失败时返回 `StoreError::Io` 并且不写主文件；
补一条注入 `take_backup_failure()` 的测试，断言文件字节未变。

**核验**：已核验。

### 2.2 `inbox.md` 缺失时 `restore_backup` 必然失败（P0）

**位置**：`crates/slip-store/src/lib.rs:841-853`、`crates/slip-store/src/fsops.rs:151`。

恢复前无条件 `fsops::backup(&path)`，而 `backup()` 第一行 `read_bytes(path)?` 对缺失文件返回
`NotFound`。最需要恢复的场景（外部删除/同步工具清空、隔离重建失败）反而恢复不了。

**建议**：`NotFound` 视为「没有当前内容需要保护」继续执行，只有其它 I/O 错误才中止。
同时先校验 `name` 形状与存在性，再创建备份，避免一次失败消耗一代备份。

**核验**：已核验。

### 2.3 备份本身不持久、可被截断、未绑定 `inbox.md`（P1）

**位置**：`crates/slip-store/src/fsops.rs:151-171`、`:268-337`。

- `backup()` 只 `create_new + write_all`，**没有 `sync_all`**；写备份途中断电会留下截断文件；
- `list_backups` 读失败时静默变成 `slips:0, preview:""`，不区分「空备份」与「读不出」；
- `restore_backup` 不解析候选字节就直接覆盖 inbox，坏备份要等事后 `reload()` 才发现；
- `parse_backup_name` 只校验形状，不要求前缀是 `inbox.md`，vault 里任意 `*.bak-*` 都可能被列出/恢复/清理；
- Windows 盘符前缀可绕过 `..`/分隔符守卫（`C:x.bak-1` 会替换基路径）。

**建议**：备份先写临时文件 + `sync_all` 再改名；恢复前解析候选并拒绝不可解析内容；
所有备份操作统一要求文件名前缀等于 `inbox.md`；路径校验改用 `Path::components()` 拒绝
`Prefix` / `RootDir` / `ParentDir`。

**核验**：已核验（机制）。

### 2.4 事务回读闭包不校验被修改行的 `extras`（P1）

**位置**：`crates/slip-store/src/doc.rs:737-773`。

`verify_roundtrip` 对已被 patch 的行直接 `continue`（字节比较跳过），记录比较又只看 `Slip`，
不含未知键 `extras`。设计 §4.3 承诺未知片段及顺序逐字保留，但事务层没有守卫，
只有 `doc.rs` 的单测覆盖。`rewrite_line` 一旦回归，用户手写的 `zz=9` 会静默消失。

**建议**：`expected` 携带 `Record`（含 extras）比较，或对每条被修改行断言
「旧行未知片段序列 == 新行未知片段序列」。

**核验**：已核验。

### 2.5 失败的写入诊断会永久残留（P1）

**位置**：`crates/slip-store/src/lib.rs:583-590`、`:741-761`。

回读闭包/删除校验失败只写 `state.diagnostics`，不降级；而 `apply` 成功后只重算
`UnknownField` 警告，从不 `diagnostics.clear()`。一次理论上的内部不变量失败之后，
后续所有成功写入仍会让 UI 顶部横幅显示「回读闭包失败」。

**建议**：成功提交后清空 `diagnostics`；这类失败要么明确降级，要么放进一次性的 `StoreError`，
不要留在持久状态里。

**核验**：已核验。

---

## 三、撤销与状态一致性（P1）

### 3.1 `SetDone` 的逆操作漏掉两种合法状态

**位置**：`crates/slip-app/src/dispatch.rs:566-574`；领域侧 `crates/slip-domain/src/state.rs:95-117`。

逆操作只认 `(true, Some(at))`，其余一律生成 `SetDone{done:false}`。但领域层明确允许：

- `done=true, done_at=None`（测试 `accepts_done_without_done_at`）：撤销「取消完成」不会恢复完成态；
- `done=true` 且 `archived_at.is_some()`：`SetDone{true}` 会清掉 `archived_at`，逆操作也不补归档。

**触发场景**：手工把 `[ ]` 改成 `[x]`（无 `done_at`）后取消完成再 `Ctrl+Z`；或在归档抽屉里取消完成再撤销。
界面提示「已撤销」，状态却没回到原样。

**建议**：按 `slip.done` 分支；原状态 `archived_at.is_some()` 时补 `Patch::Archive{at}`。
补两条回归测试。

**核验**：已核验。

### 3.2 整体替换失败时也清空撤销栈

**位置**：`crates/slip-app/src/dispatch.rs:314-320`、`:434-450`。

`RestoreBackup` 与 `IsolateAndReset` 在 `settle` 之后无条件 `undo.clear()`，包括失败结果。
`Reload` 已经采用「只有真的采纳新内容才清栈」（`:339-343`），这两条没有对齐。
备份不存在、名字非法、健康时调用 isolate（`NotDegraded`）都会让文档保持不变，却清空用户的撤销历史。

**建议**：只在 `error.is_none()` 时清栈；`NotDegraded` 尤其应保留。补测试。

**核验**：已核验。

### 3.3 `PurgeArchived` 的 panic 分支谎报「未执行」

**位置**：`crates/slip-app/src/dispatch.rs:409-413`。

`spawn_blocking` 返回 `JoinError` 时只记日志并回 `not_executed`，不进入 `Fatal`。
panic 可能发生在 `rename` 之后：磁盘已删、快照未变，却告诉调用方「未执行」。
若 panic 源于锁中毒，后续命令会在 `inverse_batch(&store.snapshot())` 处继续 panic，消费者静默死亡。

**建议**：与其它命令一致走 `fatal()`；未知状态不要用 `Rejected` 冒充 `not executed`。

**核验**：已核验。

### 3.4 空操作也进入撤销栈

**位置**：`crates/slip-app/src/dispatch.rs:258-269`。

入栈只看 `ack.result == Applied`，而 Noop 也返回 Applied（`pipeline.rs` 明确验证「noop is still a success」）。
重复提交同一文本、重复置为同一完成态、重复归档都会多出一条「按了 Ctrl+Z 什么也不发生」的步骤。

**建议**：仅在内容真的变化时入栈（比较 `content_rev`，或让 `StoreOutcome` 携带 `changed`）。
**核验**：待复现。

### 3.5 结构守卫的覆盖比声明弱

**位置**：`crates/slip-app/tests/frontend_lifecycle.rs:31-53`。

`assert_body_contains` 是纯子串匹配，把清理语句注释掉仍会通过；`summon()` 只断言了
`showArchived/showBackups/pendingRestore`，同表要求的 `notice = null`、`filter = ""`、
`filtering = false` 没断言。模块注释只承认「不检查条件分支」，没有承认注释/子串这个更宽的假阴性。

**建议**：过滤注释行后再匹配；补齐 `summon` 的三项；或改为解析语句列表。

**核验**：待复现（读取范围有限）。

---

## 四、前端：交互与数据丢失（P1/P2）

### 4.1 过滤时正在编辑的草稿被静默清空（P1）

**位置**：`ui/src/App.svelte:1504-1506` 传 `slips={visible}`；`ui/src/lib/SlipList.svelte:180-188`。

清理 effect 的条件是「`editingId` 不在 `slips` 里」。而 `visible` 包含过滤结果：用户编辑一条时按
`Ctrl+F` 输入过滤词把该条筛掉，`editingId` 与 `draft` 立即被清空，没有任何提交或提示。

**建议**：区分「条目真的消失」与「只是被视图过滤」。用 `allSlips` 判断存在性，或在清理前提交草稿。
**核验**：已核验（数据流）。

### 4.2 预览弹窗不是只读：`Ctrl+Z` 会触发真实撤销（P2）

**位置**：`ui/src/App.svelte:1005-1040`、`:1535-1560`。

`handleKeydown` 只拦 `listEditing` 与输入框焦点；预览打开时焦点在 `body` / `<pre>`，
`Ctrl+Z` 会走 `handleUndo()`。弹窗文案却写着「只读 · 不会写回」。

**建议**：`preview` 非空时在 `handleKeydown` 顶部短路；把焦点移入弹窗。
**核验**：已核验。

### 4.3 编辑失败后仍抢焦点，导致重复提交（P2）

**位置**：`ui/src/lib/SlipList.svelte:281`、`:320-332`、`:497`。

`commitEdit().then(() => refocusRow(id))` 不检查结果。失败时 `accepted=false`，编辑框仍在，
焦点却被移到行上 → `onblur` 再次 `commitEdit()`，同一个失败请求发两次，用户也离开了输入框。

**建议**：只有 `accepted` 为真才 `refocusRow`；失败时保持焦点并显示错误。
**核验**：已核验。

### 4.4 「导出」入口与操作对象不一致（P2）

**位置**：`ui/src/App.svelte:1269`。

按钮条件是 `activeSlips.length > 0`，但 `handleExport` 导出整份文件。只有归档条目时按钮消失，
用户无法导出。建议改为 `snapshot?.slips.length > 0`。
**核验**：已核验。

### 4.5 备份面板的异步竞态（P2）

**位置**：`ui/src/App.svelte:683-693`。

`toggleBackups` 的 `await bridge.backupList()` 没有代数/取消保护。快速开→关→开时旧请求可能后返回，
覆盖新结果；关闭后返回的失败还会在面板外写 `hint = "读取备份失败"`。
建议用自增 token 或 `showBackups` 校验后再赋值。
**核验**：已核验（代码路径）。

### 4.6 其它待复现项

- `inline.js` 的 `TRAILING_PUNCTUATION` 含 `(` 与 `)`，会剥掉维基式 URL 末尾的 `)`，点击 404（**已核验**表内容，场景待复现）。
- 前端 `href` 只做 `https?://` 粗校验，中键打开不触发 `onclick`，会绕过 `openUrl` 的后端协议校验（待复现）。
- 1000+ 条时每次过滤按键全量 `buildInline`，输入可能卡顿（待复现，建议记入 backlog）。

---

## 五、外壳：可观测性、热键与阻塞 I/O（P1/P2）

### 5.1 运行期换热键不落盘（P1）

**位置**：`src-tauri/src/hotkey.rs:104-136`、`src-tauri/src/system.rs:369-384`。

`switch_to_fallback` 注释明确「不写偏好」，成功只 `state.set_hotkey()`。而产品里没有「别处」可配：
`JsonPrefsStore::save_prefs` 在生产代码中没有调用者。重启后热键回到默认值，冲突横幅再次出现。

**建议**：切换成功后经 `JsonPrefsStore::save_prefs` 落盘（`Prefs` 目前只有 `hotkey`，成本极低），
并确保 `PaperViewSet` 携带该字段，避免将来视图写回时擦掉。
**核验**：已核验。

### 5.2 热键状态只取一次，换键后横幅撒谎（P2）

**位置**：`ui/src/App.svelte:1183-1190`。

注释假设「启动即定、运行期不变」，但 R116 就是在运行期换键。启动失败横幅提示「重启即可恢复」，
用户切到备选键成功后，横幅仍说未注册。建议在 `summon` / 窗口 focus 时重取，或由 Rust 侧发状态事件。
**核验**：待复现（前端调用时序）。

### 5.3 备选键与原键都失败时，托管状态仍声称旧键可用（P2）

**位置**：`src-tauri/src/hotkey.rs:119-131`、`src-tauri/src/system.rs:374-383`。

fallback 失败且恢复旧键也失败时只返回 `Err`；调用方只 `warn!`，`ShellState.hotkey` 保持旧值。
托盘 tooltip、`hotkey_status.registered`、前端横幅都显示旧键可用。建议区分「已恢复旧键」与
「当前无任何热键」，后者把 `hotkey` 置空。
**核验**：已核验。

### 5.4 启动 I/O 失败在 release 下静默崩溃（P1）

**位置**：`src-tauri/src/lib.rs:748-770`、`src-tauri/src/main.rs:2`。

`ensure_dir` 失败直接 `panic!`，外层 `join().expect(...)` 再次 panic；release 是
`windows_subsystem = "windows"`，没有控制台，错误只进 stderr。用户看到「双击没反应」。
另外 `std::thread::spawn(...).join()` 只是把 I/O 挪到线程后**阻塞 UI 等它**，并不解除启动阻塞。

**建议**：失败时不要 panic，显示窗口 + `Fatal` 状态或原生错误对话框，必要时回落到可用目录。
**核验**：已核验。

### 5.5 同步 IPC / 托盘回调里做重 I/O 或 `block_on`（P2）

**位置**：`src-tauri/src/lib.rs:181-187`、`:422-434`、`:614-626`；`src-tauri/src/system.rs:355-366`。

`backup_list` 逐份读入并解析全部备份，`vault_capacity` 读整个 `inbox.md`，`corrupt_files` 逐项
`metadata`；托盘菜单回调里 `block_on(dispatcher.get_snapshot())`。大文件或慢盘时会卡住主线程/菜单。
**建议**：改为 `async fn` + `spawn_blocking`；菜单回调只 `spawn` 任务；托盘计数用 watch channel，
不要为了计数深拷贝整份快照。
**核验**：部分已核验（`block_on`）；其余待复现。

### 5.6 release 下关键日志不可见（P2）

**位置**：`src-tauri/src/lib.rs:689-705`、`src-tauri/src/main.rs:2`。

`tracing` 固定写 stderr，release 无控制台。热键注册失败、`DurabilityUnconfirmed`、窗口丢失等
全不可见，`SLIP_LOG=debug` 也读不到。建议 `SLIP_LOG` 允许可选文件日志（滚动、限大小），
或至少提供 `OutputDebugString` 等可抓取通道。
**核验**：已核验（初始化方式）。

### 5.7 符号链接可绕过损坏证据的路径守卫（P2）

**位置**：`src-tauri/src/lib.rs:643-666`。

`guard_corrupt_path` 只 canonicalize 父目录，返回未规范化的 `candidate`，`File::open` 会跟随链接。
vault 内的符号链接/硬链接可让「只读证据」读到 vault 外的文件。建议对文件本身 `canonicalize`
后再校验前缀并 `is_file()`，或拒绝 reparse point。
**核验**：待复现。

---

## 六、领域层边界与契约（P2）

### 6.1 标签规则有两份实现，Rust 版是死代码

**位置**：`crates/slip-domain/src/tags.rs:60`、`ui/src/lib/tags.js:46`。

`extract_tags` 只有测试调用，生产展示走 JS 版。两份规则已经存在确定分歧（`U+0085`、`U+FEFF` 的空白判定），
`tools/check-ui-logic.mjs` 只测 JS 版。修改领域层标签规则不会影响界面，界面规则也没有 Rust 侧守卫。

**建议**：明确唯一归属。保留一份，或把规则表做成可比较的契约（同一批固定输入跨语言对比）。
**核验**：已核验（调用点）。

### 6.2 `frontend_contract` 只钉数字，不钉语义

**位置**：`crates/slip-domain/tests/frontend_contract.rs:33-43`。

测试只解析 `const NAME = <整数>;`，但真正会漂移的是前缀表、行拆分与 `trim` 口径。
给 Rust 增加一种前缀而不同步前端，Linux CI 仍全绿。建议用共享的固定输入集做跨实现对比，
或导出规则表而不是只导出常量。
**核验**：待复现。

### 6.3 换行/空行口径不完整

**位置**：`crates/slip-domain/src/split.rs:66-67`、`:37-41`。

- 只按 `\r` / `\n` 拆分；粘贴含 `U+2028` / `U+2029` / `U+0085` / `U+000B` / `U+000C` 的文本会得到
  一条跨多行的「单条」，浏览器却按强制换行渲染；
- 零宽字符（`U+200B`、`U+FEFF`）不参与 `trim()`，会生成「看不见内容」的纸条；
- checkbox 只剥 `"[ ] "` 等带尾随空格的形态；`- [ ]`（行尾无空格）会得到正文为 `[ ]` 的纸条。

**建议**：在领域层统一定义「换行 / 空行 / 零宽」的口径，同步前端预检；补对应测试。
**核验**：已核验（代码）；Unicode 触发待复现。

### 6.4 超限提示报的是「至少」而非真实条数

**位置**：`crates/slip-domain/src/split.rs:79-85`、`error.rs:46-48`。

超出即返回，`actual` 恒为 `MAX_LINES + 1`；文案「现在是 101 条」在粘贴 500 行时与事实不符。
代码注释写的是「至少超到几条」，但用户看不到这层限定。建议扫描完再报真实条数，或文案改成「至少」。
**核验**：已核验。

### 6.5 时间与平台的小口径问题

- `state.rs:21-31` 的往返校验用 `OffsetDateTime` 的 `PartialEq`（按时刻比较，忽略偏移），
  与设计「保留同一时间值和偏移」不完全一致（待复现）。
- `split.rs:21` 与前端粘贴清洗各剥一层前缀，组合路径可能剥两层，与领域层「只剥一层」不一致（待复现）。

---

## 七、性能与结构（P2）

> 除「单次提交 I/O 放大」已按代码路径核验外，本节其余项为代码走读推断。
> 建议先用 `perf.rs` 的现有基线与 10k 行样本测量，再决定是否投入改动。

| 项 | 位置 | 说明 | 建议 |
|---|---|---|---|
| 单次提交 I/O 放大 | `slip-store/src/lib.rs:683-727` | `read_for_conflict` 全量读 → `backup` 再读一次并全量写 → `write_atomic` 再写 + fsync；已读并校验 hash 的字节被丢弃 | 把 `read_for_conflict` 的字节交给 `backup` 复用 |
| 消费者线程深拷贝快照 | `slip-app/src/dispatch.rs:259` | 每次可撤销 `Apply` 同步 `store.snapshot()` 克隆整份文档，是主循环里唯一未下沉到 `spawn_blocking` 的 store 调用 | 让 `apply` 返回写入前快照，或提供轻量版本查询 |
| 解析每行复制两次 | `slip-store/src/doc.rs:176-184` | `build_parsed_line` 内部 `to_vec()`，调用方又 `copy_from_slice`；每行一次多余分配 | 让构造过程只分配一次 |
| 50k 行无写入性能门限 | `tools/perf.rs` | 写延迟只在 1000 行下测；默认 vault 常在 OneDrive 同步目录 | 补 10k 行写入测量，并记录配置 |
| 托盘每秒级克隆全量快照 | `src-tauri/src/lib.rs:837-858` | 1.5s 轮询克隆整份 `UiSnapshot` 只为数未完成条数 | watch channel 维护计数 |

---

## 八、优化路线图

### 批次 0：让门禁重新可信（先做，半天内）

1. 修 `check-forbidden` 的 `Include` 语义，补 `Cargo.toml` / `tauri.conf.json` 的 SelfTest 熔断用例。
2. 把 `check-ui-logic` 的退出判定移到文件末尾。
3. 修 `mutation-audit.py` 的路径推导与「变红」判据。
4. `check-ci-parity` 对未知步骤报错，并同时断言命令片段。
5. 把 `-SelfTest` 接入钩子/CI。

**验收**：分别故意破坏一条约束，确认对应脚本退出码非 0；再还原并确认全绿。

### 批次 1：数据不可丢（优先级最高）

1. `purge_archived` 备份失败即中止，不写主文件。
2. `restore_backup` 允许 `NotFound`，先校验名字再备份，恢复前解析候选。
3. 备份 `sync_all` + 名字绑定 `inbox.md` + `Path::components()` 校验。
4. `SetDone` 逆操作覆盖 `done_at=None` 与归档态；整体替换失败不清撤销栈；`PurgeArchived` 的 `JoinError` 进 `Fatal`。
5. 成功提交后清 `diagnostics`；回读闭包校验 `extras`。

**验收**：每条补一个回归测试，并做变异验证（去掉修复后测试必须变红）。

### 批次 2：日常可用性

1. 换热键落盘 + 前端刷新 + 双失败时状态置空。
2. 启动 I/O 失败不 panic，给出可见错误。
3. 同步 IPC / 托盘回调的重 I/O 下沉 `spawn_blocking`。
4. release 可选文件日志（`SLIP_LOG`）。

### 批次 3：前端与领域边界的收尾

1. 过滤时保留编辑草稿；预览态拦 `Ctrl+Z`；编辑失败不抢焦点；导出条件修正；备份面板竞态。
2. 统一标签规则归属；`frontend_contract` 从「数字」升级到「语义」。
3. 统一换行/空行/零宽口径；超限文案。
4. 性能项按实测决定是否投入，不先做无数据支撑的优化。

---

## 九、已排除的误报与有意设计

- **`commitEdit` 会把 A 的草稿写进 B（子代理报告的「数据写错条目」）— 不成立。** `beginEdit` 在
  `draft = slip.text` 之前调用 `void commitEdit()`；async 函数同步执行到第一个 `await`，
  此时 `onedit(..., draft)` 的实参已按值求值，仍是 A 的草稿。已核验。
- `Presenter` 不上锁、无 watcher、无 SQLite、无事件总线、`inbox.md` 单一写入者、逐字节保留：
  这些是**有意设计**，不是缺陷。审查中未建议引入这些机制。
- 「备份失败不阻断主文件写入」（§459）对普通写入仍然成立；本文只要求 `purge_archived` 这个
  不可逆操作例外。
- `domain` 的 `cfg!(windows)` 换行导出是 R55 的有意设计，仓库文档「零平台依赖」的表述不准确，
  但实现本身无需改动。

---

## 十、执行记录

- 逻辑层测试：`cargo test -p slip-domain -p slip-store -p slip-app --all-targets --offline`
  → **10 个目标 / 286 项通过 / 0 失败**（Linux，2026-11-12）。
- 未执行 Windows 外壳构建与 `tools/verify-hotkey.ps1`（需要 Windows 交互桌面）。
- 未执行本次新建议的回归测试——它们尚不存在；批次 1 落地时应逐条补上并做变异验证。


---

## 十一、复核修正与批次 0 执行记录（2026-11-12 当日追加）

本节由修复执行方回读源码后追加。**本文第三章整节已过期**（它描述的是 85b9802 之前的状态，
两个提交相差 22 秒）；下面同时修正几处与源码不符的结论，避免后来者照着做重复或错误的工作。

### 11.1 第三章逐条复核

| 条目 | 复核结果 |
|---|---|
| 3.1 `SetDone` 逆操作漏状态 | 已修，`dispatch.rs` 按 `slip.done` 分支，归档态补 `Patch::Archive` |
| 3.2 整体替换失败也清撤销栈 | 已修，三处均为 `if error.is_none()` |
| 3.3 `PurgeArchived` 的 `JoinError` 谎报未执行 | 已修，走 `fatal()` |
| 3.4 空操作进撤销栈 | 已修，`run_batch` 用 `content_rev` 判 `changed` |
| 3.5 `assert_body_contains` 是纯子串匹配 | **事实错误**：函数叫 `assert_body_has_statement`，已跳过注释行并要求整行相等；`notice` / `filter` / `filtering` 三项都在断言列表里 |

### 11.2 需要降级或修正的条目

- **§7（purge 的逐字节保护只有一条单测）**：不成立。`purge_removes_exactly_one_line_and_keeps_the_rest_byte_identical`
  已经逐行比对删除前后的字节。真正缺的是**事务层自身的闭合断言**，按 P2 处理。
- **§4（门禁对等性只检查一个方向）**：不准确。`$missingInHook` 确实检查了 CI→钩子方向。
  真实问题有三点：归一失败被 `Where-Object { $_ }` 静默丢弃、`$hookOnly` 是空死代码、
  命令片段只比对 `--workspace` 一处。
- **§15（过滤视图清草稿）**：触发面比文中描述的还窄。编辑期间 `handleKeydown` 直接 return，
  按不到 Ctrl+F；点过滤框会先触发 blur 提交。现实入口只剩「编辑中外部改文件 + 静默刷新筛掉该条」。
  **本轮不改**：它会牵出「过滤导致编辑框卸载时草稿算提交还是丢弃」的语义问题，不应顺手决定。
- **§11（编辑失败抢焦点导致重复提交）**：Enter 路径无害（编辑态下 `itemEls[index]` 未绑定，
  `focus()` 是 no-op）。真正有后果的是 **Tab 路径**与 `beginEdit` 里的 `void commitEdit()`，
  两处都会把同一份失败草稿再发一次并静默丢弃原草稿。

### 11.3 本次实测到的环境事实

- 逻辑层基线：**10 个目标 / 304 passed / 5 ignored / 0 failed**（本文 §十 的 286 已过期）。
- 外壳 lib 测试：24 passed。
- `tools/mutation-audit.py` 在修复前**完全失效**：`REPO = r"D:\Dev\simple-todo"` 与
  `wsl.exe -d Arch` 硬编码；本机发行版是 `arch_lagacy`，仓库在 ext4。
- `check-forbidden.ps1` 的 `Get-SourceIndex` 在 `\\wsl.localhost\...` 下用
  `(Get-Location).Path` 会带 `FileSystem::` 前缀，`Directory.EnumerateFiles` 抛错，
  **索引是空表、所有检查恒绿**。这与 `-Include` 写错是两个独立的假绿来源。
- **Windows PowerShell 5.1 不是可用的门禁环境**：`ConvertFrom-Json` 受 BOM 影响抛异常；
  中文按 GBK 解码导致关键词匹配不上（`check-ci-parity` 只识别出 2 个关口）。
  门禁证据只认 `pwsh 7`。`check-doc-sync.ps1` 已改为读取时剥 BOM，但 5.1 的 GBK 问题不修——
  钩子与 CI 都显式用 `pwsh`。

### 11.4 批次 0 已完成（门禁）

四条门禁修复全部落地，每条都做了「故意破坏 → 退出码非 0 → 还原」的验证：

1. `check-forbidden.ps1`：`-Include` 拆成扩展名 / 精确文件名两种语义；
   新增 `Split-IncludePatterns` / `Test-IncludeMatch`；`-SelfTest` 增加**端到端**用例
   （临时 `Cargo.toml` 写 `notify` 必须被抓到）。另修 `Get-Location.ProviderPath`。
   破坏验证：根 `Cargo.toml` 注入 `notify = "6"` → exit 1。
2. `check-ui-logic.mjs`：退出判定移到文件末尾；新增「最后一次 `check(` 必须在退出判定之前」
   的结构守卫。破坏验证：末尾追加必失败断言 → exit 1。
3. `mutation-audit.py`：路径由 `__file__` 推导，直接调 `cargo`；判据收紧为
   `test result: FAILED` 才算红，编译失败记 `[INVALID]` 并计失败；增加基线预检。
   实测 10/10 变红、12 秒、还原后全绿。破坏验证：把一条变异改成语法错误 → `[INVALID]`、exit 1。
4. `check-ci-parity.ps1`：补三条关口（常量 / 文档同步 / 门禁对等性）；未识别步骤默认 FAIL，
   白名单按原始步骤名显式列出；命令片段双向断言（`--all-targets`、`-D warnings`、
   `--workspace`、`fmt --all --check`、`-SelfTest`）。块提取会剥掉注释行——
   第一版没剥，命令里删掉 `-SelfTest` 仍判绿，是假阴性。
5. `-SelfTest` 已接入钩子与 CI。
6. `check-doc-sync.ps1`：读取时剥 BOM；新增预算一致性断言（脚本 `$cssLimit` / `$jsLimit`
   与 `docs/handover.md`、`.githooks/pre-commit`、`ui/src/lib/tags.js` 的 KB 数字比对）。
   破坏验证：把 handover 的 JS 预算改回 80 KB → exit 1。

顺带清掉三处预算漂移：`docs/handover.md`（70→88 KB）、`.githooks/pre-commit` 注释
（CSS 10→14、JS 70→88）、`ui/src/lib/tags.js`（70→88）。
