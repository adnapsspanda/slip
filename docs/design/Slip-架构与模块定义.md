# Slip · 架构与模块定义 v11（实现基线）

> v11 依据：v10 修订稿 + 第六轮审查合并。
> 相对 v10：新增 4 条必修（清空输入收敛条件、Isolated 死锁出口、视图事件版本、未修改行字节断言）、6 条澄清、2 处降级/切分。
> 延续：单队列；v1 无 SQLite；无 watcher；CLI/MCP 不进 v1；内容提交点 = 原子替换成功。
> 状态：可进入实现。窗口形态与性能由 spike/实测决定（§2.4 已降级为待验证假设）。

---

## 0. v10 → v11 修订记录

| # | 类型 | 问题 | v11 定义 |
|---|---|---|---|
| 1 | **必修** | `applied` + emit 失败 + 补偿失败时，按 §6.5 三条件会清空输入，但界面无新纸条 | 清空增加第四条件：**本地快照已收敛到 `ack.ui_rev`**（§6.5） |
| 2 | **必修** | `Isolated` 状态下现有文件再次损坏 → 永远 `Blocking`，无出口 | 禁止 rename 的对象精确为"**已成功解析的文件**"；无效文件允许再次隔离为 `corrupt-2`（§3.4） |
| 3 | **必修** | 视图事件是否递增 `ui_rev` 未定义 → DPI/拖拽更新可能被前端去重丢弃 | 凡生成并发射新 `UiSnapshot` 均递增；`GetSnapshot` 不递增（§6.3） |
| 4 | **必修** | 回读闭包只比较记录，无法发现"全文重序列化"改写了非记录行 | 候选文档必须**由原始行数组局部替换构造**；闭包断言未修改行字节相等（§4.3、§6.1、I13） |
| 5 | 澄清 | 四类状态组合无展示优先级 | 检查顺序与展示优先级分开定义（§4.7、§5） |
| 6 | 澄清 | 捕获框归属层未定义 | 归 `shell` 的独立窗口，不进 `UiSnapshot`、不进队列（§1.1、§6.4） |
| 7 | 澄清 | `[X]` 大写会致整库 `Degraded` | 解析接受 `x`/`X`，写出统一小写（§4.2） |
| 8 | 澄清 | "键顺序不参与语义"在 v10 丢失 | 补回（§4.2） |
| 9 | 澄清 | `GetSnapshot` 的 reply 类型无法承载查询错误 | reply 改为 `Result<UiSnapshot, QueryError>`（§6.1） |
| 10 | 澄清 | §3.4 B-5 混用 `Degraded`/`Normal` 两个枚举 | 拆写为 availability 与 recovery 各自取值（§3.4） |
| 11 | 澄清 | `StoreError` → `Ack.result` 无映射表 | 补映射表（§6.5） |
| 12 | **降级** | §2.4 把"spike 失败只换 shell"写成结论 | 降级为**待验证假设**，列明会连带变动的边界（§2.4、§12） |
| 13 | **切分** | 25 条不变量压在第一步 | §12 切分 M0/M1/M2，明确各阶段验收子集 |

---

## 1. 领域模型

### 1.1 三层状态与捕获框归属

```
Content（进 md）      Slip
Presentation（会话）  PaperView、Prefs
Frame（一帧）         desired / target / applied Rect
```

**捕获框不属于以上任何一层**：

- 由 `shell` 拥有的独立窗口（与纸片层分离），负责原生 `show/hide/focus`；
- 其打开/关闭状态**不进 `UiSnapshot`**，`presenter` 不感知；
- 输入内容、IME、粗校验完全在前端本地，**不进命令队列**；
- 只有"提交"进入队列（§6.4）。

理由：捕获框需要焦点与输入，纸片层通常点击穿透；两者窗口属性相反，混在一个窗口里会让命中与焦点互相干扰。

### 1.2 类型

```rust
pub struct SlipId(Uuid);            // UUIDv7，小写连字符
pub struct ContentRev(u64);
pub struct RequestId(String);

pub struct Slip {
    pub id: SlipId,
    pub text: String,
    pub done: bool,
    pub created_at: OffsetDateTime,
    pub done_at: Option<OffsetDateTime>,
    pub archived_at: Option<OffsetDateTime>,
}

pub enum Patch { SetText(String), SetDone { done: bool, at: OffsetDateTime }, Archive { at: OffsetDateTime } }
pub enum Mutation { Append(Slip), Patch { id: SlipId, patch: Patch } }
pub enum Transition { Changed, Noop }
```

### 1.3 状态转换（应用行为，非文件约束）

| 操作 | 前置 | 结果 |
|---|---|---|
| `SetText(t)` | id 存在；`t` 过 `normalize_text` | 更新 text，保留其他字段；规范化后与旧值相同则 `Noop` |
| `SetDone{true, at}` | 未 done | `done=true, done_at=Some(at), archived_at=None` |
| `SetDone{true, at}` | 已 done | `Noop`；不补写缺失的 `done_at`，不重写原时间 |
| `SetDone{false, at}` | id 存在 | `done=false`，清 `done_at`、`archived_at`；三字段本来就如此则 `Noop` |
| `Archive{at}` | done 且未有效归档 | `archived_at=Some(at)` |
| `Archive{at}` | 已有效归档 | `Noop`；保留原归档时间 |
| `Archive{at}` | 未 done | `Err(ArchiveRequiresDone)` |
| `Append(s)` | `validate_slip(s)` 通过且 id 不存在 | 追加 |

跨字段异常组合允许从文件载入，并允许文本编辑保留它们。应用不自动"修复"元数据；`Append` 不承担跨字段修复。

### 1.4 校验：格式闭包与人类可编辑性

```rust
pub fn normalize_text(raw: &str) -> Result<String, DomainError>;
pub fn validate_slip(s: &Slip) -> Result<(), DomainError>;
```

`normalize_text`：去首尾 Unicode White_Space；非空；`1..=2000` 个 Unicode 码点；结果不含 CR/LF；不合并内部空白，不做 Unicode 归一化。前端只做粗校验，Rust 是最终裁决者。

`validate_slip` 必须验证**文件真正使用的格式**：

| 检查 | 错误 |
|---|---|
| text 非空、长度合法、无 CR/LF，且等于 `normalize_text` 的结果 | 对应文本错误 / `TextNotNormalized` |
| id 非 nil、UUID 版本为 7；字符串入口只接受小写连字符规范形式 | `InvalidId` |
| 每个时间均可用 `time::Rfc3339` 格式化，再用相同格式解析，保留同一时间值和偏移 | `TimestampNotSerializable` |

不能用"已经是 `OffsetDateTime`"替代第三项：RFC3339 格式化受年份上限与偏移秒等边界约束，以实际 format→parse 结果为准。

**不检查** `created_at` / `done_at` / `archived_at` 的先后；**不检查** `done` 与时间字段之间的蕴含。时间可回拨，外部只勾选或取消勾选仍可载入。

应用提交采用"候选文档 → 序列化字节 → 同一解析器回读 → 比较领域记录 + 比较未修改行字节"的检查闭包（§6.1）。文件合法性是文法、身份、文本及可往返格式的合法，**不是自动推断用户意图**。

### 1.5 时间语义

- 时间戳是记录信息，不是顺序判据；操作顺序由单队列保证；
- 时间可回退，显示层原样显示（不修正、不排序）；
- `SetDone{false}` 清时间字段是应用行为，不是文件约束。

### 1.6 拆条口径

- 按 `\r\n|\r|\n` 切；不对 `raw_text` 整体施加 2000 限制（2000 是**单条**上限）；
- 去空白后为空的行丢弃；条数 `1..=100`；全空 → `EmptyCapture`；
- 一次捕获一次 `apply`、最多一次 rename、`content_rev` +1；
- `RequestId` 用于 ACK 关联，**不参与去重**。

### 1.7 Noop

全批 Noop → 不写盘、不备份、不增 rev、不 fsync；返回当前 outcome，允许发布同状态快照。

### 1.8 PaperView 与异常元数据的展示

有效归档的唯一计算方式：

```rust
effective_archived     = slip.done && slip.archived_at.is_some();
visible_in_default_view = !effective_archived;
```

- 外部把归档条目改成未完成，即使 `archived_at` 残留，也在默认视图显示；读取不改文件。
- 完成时间缺失时显示"完成时间未记录"；未完成条目残留 `done_at` 时不把它当作当前完成状态。
- 从未完成改为完成时，按 §1.3 清残留 `archived_at`；归档必须由独立操作表达。
- 内存排序由唯一的 `Vec<SlipId>` 决定；`DragEnd` 的 `order` 是目标插入位置，**不维护第二套持久化 `order: u32`**。
- 载入偏好时去重、移除已不存在的 ID；新 ID 按文档记录顺序追加；`collapsed` 同样只保留现有 ID。
- 不持久化坐标；恢复 `order`/`collapsed`/`dock_side` 后由 `dock` 重算。溢出滚动，不截断记录。
- `ui-state.json` 缺失或损坏只影响展示偏好，不影响 Markdown 载入。

---

## 2. 结构

```
slip/
├─ crates/
│  ├─ slip-domain/   # §1；零 I/O、零 tauri
│  ├─ slip-store/    # 唯一磁盘写；零 tauri；v1 无 SQLite
│  │  └─ src/{lib,doc,prefs}.rs
│  └─ slip-app/      # Tauri 组装、调度器、commands
│     └─ src/{dispatch,commands,dock,shell,presenter,main}.rs
└─ ui/src/lib/bridge.ts
```

### 2.1 store 零 tauri 与必要依赖

`slip-store` 不依赖 `tauri`。直接依赖限于 `domain`、`uuid`、`time`、`serde`、`serde_json`、`tempfile`、`blake3`、`tracing`。

- `serde_json` 只用于 `ui-state.json`；`PrefsStore` 的类型不依赖 `presenter`、`Frame` 或 Tauri；
- `Hash` = 磁盘原始字节的 **BLAKE3** 摘要；BOM、CRLF、普通行及非法 UTF-8 非记录行全部参与计算；
- `time` 启用实际需要的 parsing/formatting 能力；版本与 feature 在 `Cargo.toml` / `Cargo.lock` 固定；
- CI 检查 `cargo tree -p slip-store` 无 `tauri`；白名单针对直接依赖，不禁止正常传递依赖。

### 2.2 v1 无索引

无 `rusqlite` / `.index.sqlite` / `source_hash` / `IndexStale`。`StoreSnapshot` 由内存构建。启动全量 parse 一次。"千条毫秒级"是**待实测目标**（§9.6），不是保证。

### 2.3 前端 bridge、首订阅与通知补偿

```ts
export async function subscribeSnapshot(fn: (s: UiSnapshot) => void): Promise<() => void>;
export async function getSnapshot(): Promise<UiSnapshot>;
export async function capture(text: string, requestId: string, draftVersion: number): Promise<Ack>;
export async function complete(id: string): Promise<Ack>;
export async function toggleCollapse(id: string): Promise<Ack>;
export async function reloadVault(): Promise<Ack>;
export async function isolateAndReset(): Promise<Ack>;
```

```ts
let last: UiSnapshot | undefined;
const accept = (s: UiSnapshot) => {
  if (!last || s.ui_rev > last.ui_rev) { last = s; render(s); }
};
const unlisten = await subscribeSnapshot(accept);   // ① 订阅必须等待实际建立
accept(await getSnapshot());                        // ② 再取首个全量
```

事件与查询共用 `accept`，旧查询结果不能覆盖新事件。

**通知补偿**：最终 ACK 携带本次处理生成的 `ui_rev`。前端若尚未接纳该版本，则补一次 `getSnapshot`，经同一个 `accept` 接纳。补偿**只读快照，绝不重试 mutation**。补偿失败时保留当前界面与草稿，显示可再次同步的提示——**此时输入不得清空**（§6.5）。

组件只访问 bridge；bridge 承担传输、应答关联、订阅与版本接纳，不实现领域校验、文件解析或业务状态转换。

### 2.4 虚拟纸片 vs 每纸一窗（待验证假设，非结论）

| | 1 个置顶层 + N 虚拟纸片（首选） | 每纸一窗（备选） |
|---|---|---|
| 命中/穿透/焦点 | 自己实现，主要风险 | OS 白给 |
| 几何权威 | 单点 | 分散 |
| 拖拽/层叠/任务栏 | 自己实现 | 白给 |
| 窗口初始化时序 | 无问题 | 必须 event loop 建窗 |

**这是假设，不是已定回退方案。** 若 §9.7 spike 失败，**不只 `shell` 会变**，以下边界同时受影响，允许修订：

- `dock::layout` 的输出语义：从"窗口内相对几何"变为"每窗口的绝对屏幕坐标"；
- `MonitorSnapshot` 的分发：从"一次给一个窗口"变为"按纸片分发到多个窗口"；
- `presenter.emit` 的合成：从"一份 `Vec<Rect>`"变为"跨窗口几何集合 + 窗口生命周期"；
- `ViewEvent::DragEnd` 的语义、焦点策略、窗口创建/销毁顺序。

spike 时即按 `presenter` 输出 `Vec<PaperRender>{ id, rect, content, state }` 的边界实现，**以便把改动限制在 `shell` 与 `dock` 的输出适配层**；若实测连这层也无法隔离，则明示返工范围，不假装"只换壳"。

---

## 3. Store 与偏好

### 3.1 内容接口与快照

```rust
pub struct StoreSnapshot {
    pub content_rev: ContentRev,
    pub slips: Vec<Slip>,
    pub health: Health,
    pub needs_reload: bool,
    pub recovery: RecoveryState,
    pub diagnostics: Vec<Diagnostic>,   // 路径、行号、原因；不是第二份内容
}
pub struct StoreOutcome {
    pub snapshot: StoreSnapshot,
    pub result: Result<ContentRev, StoreError>,
}
pub trait Store: Send + Sync {
    fn apply(&self, mutations: &[Mutation]) -> StoreOutcome;
    fn reload(&self) -> StoreOutcome;
    fn isolate_and_reset(&self) -> StoreOutcome;
    fn snapshot(&self) -> StoreSnapshot;
}
pub enum StoreError {
    Conflict { current_hash: Hash }, NeedsReload, ReadOnly,
    Domain(DomainError), Blocking(BlockingError), Io(IoKind), NotDegraded,
}
```

正常业务错误返回一致快照；`panic`/`JoinError` 不属于这个契约（§6.2），**禁止伪造 `StoreOutcome` 后继续写**。

`content_rev` 进程内单调，**初始载入 = 1**（0 表示尚未载入）。成功的内容修改、有效 reload、恢复创建、接纳外部修复各 +1；失败与 Noop 不增加。健康/诊断变化不增 `content_rev`，但仍发布 `ui_rev`。

### 3.2 内部串行化

`Mutex<LoadedDoc>`；锁覆盖"读 doc → 应用 mutation → 候选校验 → 校验 hash → 备份 → 写 tmp → sync → rename → 更新内存 → rev+1"。

### 3.3 线程模型

同步 I/O 一律在 `spawn_blocking` 内执行（含 `save_prefs`）。不得在 UI 线程调用。

### 3.4 隔离、重建与启动缺失文件

```rust
pub enum RecoveryState {
    Normal,
    Isolated { corrupt_path: PathBuf, isolated_at: OffsetDateTime },
}
```

下列"**接纳文件**"均指：替换 `LoadedDoc`/`slips`/原始字节 hash，重新计算 `UnknownField`，清旧阻断诊断与 `needs_reload`，置 `Normal`/`ReadWrite`，`content_rev +1`。

**核心约束（精确化）**：不得对**已成功解析的文件**执行 rename。对读得到的**无效**文件允许再次隔离。

#### A. `recovery == Isolated`：只重试重建，不回溯原路径

1. 对 `inbox.md` 用 `create_new(true)`。成功 → 接纳空文档。
2. `AlreadyExists` → 只读一次现有文件：
   - **有效** → 接纳该文件（不 rename）；
   - **无效** → **再次隔离为 `inbox.md.corrupt-<ts>-2`**（重名递增，绝不覆盖），然后重试步骤 1；若重试仍失败 → 保持 `Isolated`，返回 `Io`，保留两份证据；
   - 读失败 → 返回 `Io`。
3. 其他创建错误 → 保留 `corrupt_path` 与 `Isolated`，返回 `Io`。

> **v10 的死锁**：A-2 遇到无效文件只返回 `Blocking`，且 Isolated 分支禁止 rename，用户再点仍是同一路径 → 永远出不去。v11 用"对无效文件可再隔离"打开出口。

#### B. `recovery == Normal`：先检查当前可用性

1. 当前 `ReadWrite` → 立即 `NotDegraded`；不读盘、不移动文件。（连续两次成功隔离后的第二次调用，唯一结果）
2. 当前 `Degraded` → 重读实际路径。读失败且非 `NotFound` → `Io`，不 rename。
3. 读取成功且 parse 成功 → 外部已修复，**接纳该文件**，不隔离。
4. 读取成功且 parse 失败 → rename 到不覆盖的 `inbox.md.corrupt-<unix_ms>`（重名递增 `-2/-3`），成功后置 `Isolated`，转 A。
5. 路径 `NotFound` → 无可隔离源文件，直接 `create_new`：
   - 成功 → 接纳空文档；
   - `AlreadyExists` → 按 A-2 处理；
   - 其他错误 → **availability 保持 `Degraded`，recovery 保持 `Normal`**（不可混写为"保持 Degraded/Normal"）。

**提交点**：重建的提交点是 `create_new` 成功，不是旧文件的 rename。空文件创建成功后的 sync 失败只附 `DurabilityUnconfirmed`，**不得描述为"尚未创建"**。创建前先在内存准备好空文档状态。

**诊断与警告**：接纳外部修复文件时重新计算其 `UnknownField`，不能一律清空仍存在的警告。接纳新空文档时清旧警告、旧解析诊断与恢复标记；本次新发生的持久化警告保留。

**启动规则**：路径不存在 → `create_new` 初始化空文件；`AlreadyExists` → 重读。已有 corrupt 文件始终保留，不按时间猜测并自动导入；启动时在 UI 提示"存在 N 个隔离文件"并提供打开目录。`RecoveryState` 仅进程内维护，重启后按实际路径载入。

**运行中 reload 遇到 `NotFound`**：不自动建文件，保留旧内容、进入 `Degraded`，等待用户明确重建。

**首次启动即坏文件**：返回空内容 / `Degraded` + 文件路径、行号及原因，**不返回部分解析结果**。初始化读盘或创建失败同样给空内容 / `Degraded` + I/O 诊断。

**正常 reload 成功**：完整接纳文件及 hash，清 `needs_reload`，恢复 `Normal`/`ReadWrite`，重算警告。**失败**：保留最后有效内容、hash 与隔离证据，只更新门禁与诊断。

重读检查不能消除外部编辑器在检查与 rename 之间的竞态；§4.8 的边界同样适用于恢复操作。

### 3.5 偏好存储与单一拥有者

```rust
pub struct Prefs { pub order: Vec<SlipId>, pub collapsed: Vec<SlipId>, pub dock_side: DockSide }
pub trait PrefsStore: Send + Sync {
    fn load_prefs(&self) -> Prefs;
    fn save_prefs(&self, p: &Prefs) -> Result<(), PrefsError>;
}
```

- 只有 `slip-store` 执行偏好 I/O；`serde_json` 编码，独立于 Markdown 事务，**不增 `content_rev`**；
- `presenter`/消费者拥有当前 `Prefs`；前端只发 `Collapse`/`DragEnd`/`DockSide` 意图，**不提交整份旧 Prefs 覆盖新状态**；
- 展示变化先更新内存并发布；折叠、排序、dock_side 都置 dirty；拖拽过程中不写；
- 消费者合并最后一次变更后 1s 的保存请求。定时消息是 **`FlushPrefs`，不携带数据**；处理时取消费者当前的完整偏好，避免旧计时器保存旧副本；
- 保存走 `spawn_blocking`；同目录 tmp + 原子替换，**不 fsync**。失败仅日志，`dirty` 保留；下次变更或正常退出时再试一次，**不循环重试**；
- 损坏时尝试保留为 `ui-state.json.bad-<unix_ms>`，重名不覆盖；隔离失败仍用默认偏好且不阻塞 Markdown；
- `load_prefs` 只在初始化调用一次；`getSnapshot` 不反复读偏好或重复隔离坏文件；
- 所有偏好 I/O 失败**不改变 Markdown 的 ACK**；
- 偏好任务 panic → 停止本进程后续偏好 I/O、保留日志，**内容操作继续**；不反复访问已故障的 `PrefsStore`，也不把它升级为 Markdown 保存失败。故障域与 `LoadedDoc` 独立。

---

## 4. Markdown 真源

### 4.1 行格式与字符集合

```text
- [ ] 买发票 <!-- slip:v1;id=019a9f3c-2b70-7d84-a1c6-8e3f5b0d2741;created=2026-11-10T11:49:14+08:00 -->

line     := checkbox SP encoded_text SP marker
checkbox := "- [" (" " | "x" | "X") "]"
marker   := "<!-- " "slip:v1" (";" kv)* " -->"
kv       := key "=" value
key      := [a-z][a-z0-9_]{0,31}
value    := value_char+
```

- `SP` 是一个 ASCII 空格；
- `value_char` 是任意 Unicode 标量，排除 `;` `=` `<` `>`，以及 `char::is_whitespace` 或 `char::is_control` 为真的字符（不用含义不明的 `VCHAR`）；
- v1 **不提供 kv 转义**；
- 头部 `slip:v1` 是固定字面量，不是 kv；
- 定位记录 marker 用**最后一个** ` <!-- slip:`，且必须形成行尾完整 marker；不能用任意 HTML 注释作为记录头；最后一个不完整时**不回退尝试更早的 marker**，直接 `InvalidRecord`；
- `encoded_text` 是 checkbox 后与 marker 前的正文。写出时先把 `&` 编为 `&amp;`，再把 `<` 编为 `&lt;`；解码反序（先 `&lt;`→`<`，再 `&amp;`→`&`）；
- UTF-8；文件开头若有 BOM 则只在解析视图中去除。Noop 不重写 BOM；真正提交时统一输出无 BOM，除此例外遵守 §4.3 的原始字节保留。

### 4.2 字段、记录候选与唯一解析结果

| 键 | 必填 | 值约束 | 重复 |
|---|---|---|---|
| `id` | 是 | UUIDv7 小写连字符 | 拒绝 |
| `created` | 是 | `time::Rfc3339` 解析，且能按相同格式再写出 | 拒绝 |
| `done_at` | 否 | 同上；缺失与 `done` 值无关 | 拒绝 |
| `archived_at` | 否 | 同上；缺失与 `done` 值无关 | 拒绝 |
| 其他 | 否 | 满足 `value_char+` | 允许，按有序片段逐个保留 |

**键顺序不参与语义**：解析时按名取值，重写只改需要变更的已知键并原位替换，未知片段顺序逐字保留。

**记录候选**：行从字节前缀 `- [` 开始，并含 ASCII 片段 ` <!-- slip:`。普通标题、说明、代码或无该片段的 todo 不是候选。

| 情况 | 判定与行为 |
|---|---|
| 非候选行，含非 UTF-8 非候选行 | 原始字节保留，不导入 |
| 候选行含非法 UTF-8 | `InvalidRecord`，`Degraded`（不能把损坏记录静默当说明行隐藏） |
| 候选行 checkbox、marker、key/value 结构不合法 | `InvalidRecord`，`Degraded` |
| 头部是 `slip:` 的其他版本 | `UnsupportedVersion`，`Degraded`；不猜测迁移 |
| 必填键缺失、已知键重复、已知值格式非法 | `InvalidRecord`，`Degraded` |
| 文本解码、trim 后为空或超 2000 码点 | `InvalidRecord`，`Degraded` |
| 同一 ID 出现多次 | `DuplicateId`，`Degraded` |
| 有效未知键 | `UnknownField`，继续读写，保留全部原始片段及顺序 |
| 有效记录在下次成功重载中消失 | 从内存移除，不复活 |

- `extra=a;b` 切为 `extra=a` 与无等号的 `b` → 唯一结果 `InvalidRecord`；
- `extra=a;b=c` 是两个 kv，不是一个含分号的值；文法无法表达的意图不作推测；
- `[X]` 与 `[x]` 均视为完成；**写出时统一小写 `x`**（仅在重写该行时生效，未修改行保留原字节）。

### 4.3 解析与局部重写

- 解析得到的领域 text 是正文解码并按 Rust `trim` 后的结果，再走 `validate_slip`；解析本身不写文件；
- **未修改行保留原始字节、原始终止符和 metadata**；仅文件 BOM 按 §4.1 显式处理；
- **候选文档必须由"原始行数组的局部替换"构造，禁止全文重新序列化**（见 §6.1 闭包断言）；
- 只有 `Changed` 的 `Patch` 才重写对应行；`Noop` 不触碰原始行。重写行使用规范化正文，沿用原行终止符；
- 修改已知 metadata 时原位更新，必要时追加；清字段即移除该已知 kv。未知片段及其相对顺序、重复项逐字保留；
- 已知字段在新记录中的序列化顺序固定为 `id`、`created`、`done_at`、`archived_at`；不存在的可选字段不输出；已有行只改需要变更的已知字段；
- "往返无损"分两层：**领域值往返相等**与**未修改行字节保留**。不承诺修改行保留多余空格、旧时间文本拼写、`[X]` 大小写或 BOM。

### 4.4 内容提交点与持久性

**提交点 = 同目录临时文件原子替换 `inbox.md` 返回成功。**

| 阶段 | 失败语义 |
|---|---|
| 候选校验、读盘比较、写 tmp、`tmp.sync_all`、替换失败 | 未提交；正常返回对应错误；内存与 rev 不变 |
| 替换成功后目录 sync 失败 | 已提交，`DurabilityUnconfirmed` |
| 替换成功后正常可处理的通知等失败 | 已提交；不返回 rejected，不回滚文件 |
| 任意阶段 panic，无法得到可信 `StoreOutcome` | 当前请求 `unknown`，进入 §6.2 Fatal，**不能谎称未提交** |

临时文件在 vault 目录内创建。完成候选内存、回读结果及 hash 的构建后再执行替换；成功后仅安装预备状态、增加 rev，并处理持久化确认，减少提交后的可失败工作。

Windows v1 不提供父目录同步确认，不宣称断电安全；Unix 尝试目录同步。**原子替换、提交应答、断电持久性是三个不同的保证**。恢复创建空文件的提交点另见 §3.4，不把多步骤隔离伪装成同一原子事务。

### 4.5 备份与 warning 生命周期

先校验磁盘原始 hash，再从刚确认的旧内容生成 `inbox.md.bak-<unix_ms>`；重名递增 `-2/-3`，**以不覆盖方式创建**。

每次成功备份后尝试保留最新 5 代，启动时再清理一次。备份创建或清理失败不阻断主文件写入，记 `BackupFailed`；下次备份与清理都成功后清除。

- `UnknownField` 在成功解析当前文档后重新计算；
- `DurabilityUnconfirmed` 在本次持久化确认失败时置位，之后一次有相应同步确认的成功提交可清除；Windows 未实施目录确认本身属平台保证边界，**不伪造一次失败事件**；
- **Noop 不清除任何依赖实际 I/O 成功才能清除的警告**。

### 4.6 追加与换行

- 读取接受 CRLF、LF、CR，逐行保存原字节与原终止符；
- 默认追加分隔符 = **第一个实际出现的终止符**（不按出现次数投票）；没有任何终止符则用 LF；
- 向非空文件追加时，若原末行无终止符，先补上述分隔符；新记录之间都使用该分隔符；
- 新末行继承追加前文件的"末尾是否有换行"属性；该规则适用于一批 N 条，不只是单条；
- 空文件或仅 BOM 文件视为逻辑空文档：N 条记录以 LF 连接，末条不加终止符；
- 修改已有记录沿用其原终止符；普通行与其余记录的混合换行保持不变。

### 4.7 写入门禁与 needs_reload

**提交条件**：`runtime == Running` && `availability == ReadWrite` && `!needs_reload`。后端必须独立检查，按钮状态不是唯一防线。

**检查顺序**（决定返回哪个错误）：

1. `needs_reload == true` → `NeedsReload`，不读盘、不写盘；
2. `availability == Degraded` → `ReadOnly`，不应用 mutation；
3. 在候选副本应用并校验全部 mutation；全批 `Noop` 返回当前状态，**不读盘**；
4. 有 `Changed` 时读盘比较原始 hash；不同则置 `needs_reload = true` 并返回 `Conflict`，不写任何文件；
5. 校验一致才进入备份与提交。

**展示优先级（决定文案与主操作）**：`Fatal` > `Isolated` > `Degraded` > `needs_reload` > 正常。

> 两者顺序不同是故意的：检查顺序保证返回码语义稳定；展示优先级保证用户看到最需要处理的问题。实际使用中 `Degraded` 已禁用提交，因此很少出现"检查走 needs_reload、界面显示 Degraded"的错位。

`needs_reload` 清除：成功 reload、接纳外部修复、成功重建。失败 reload **保留它**并更新诊断；解析或运行中读盘失败则显示 `Degraded` 与最后有效内容。

**`Noop` 没有检查磁盘，不表示磁盘一定未变**；v1 无 watcher，界面只反映最后一次观察。"真实可写性"不包含对未知外部编辑的提前预测。

### 4.8 冲突承诺边界

能发现"校验时刻已存在的变化"；不能保证"校验后到 rename 前"的写入不丢。文案不得写"绝不覆盖外部修改"。

---

## 5. Health 与运行时状态

```rust
pub enum Availability { ReadWrite, Degraded }
pub enum Warning { BackupFailed, UnknownField { line: usize }, DurabilityUnconfirmed }
pub struct Health { pub availability: Availability, pub warnings: BTreeSet<Warning> }

// 属于 app/UI，不属于 Markdown 或 StoreSnapshot：
pub enum RuntimeStatus { Running, Fatal { reason: String } }
```

`Degraded` 表示当前磁盘内容/读取状态不能用于正常写入；`Fatal` 表示当前进程的运行状态不再可信，**不能冒充"文件已损坏"**。`UiSnapshot` 同时包含 `health`、`needs_reload`、`recovery`、`diagnostics`、`runtime`。

**状态组合的展示契约**（实现者不得自行发挥）：

| runtime | availability | needs_reload | recovery | 文案 | 主操作 | 提交 |
|---|---|---|---|---|---|---|
| Running | ReadWrite | false | Normal | — | — | ✅ |
| Running | ReadWrite | **true** | Normal | 文件已在外部修改 | 重载 | ❌ |
| Running | **Degraded** | any | Normal | 文件无法解析，已进入只读 | 重载 / 隔离并新建 | ❌ |
| Running | Degraded | any | **Isolated** | 已隔离旧文件，等待重建 | 重试重建 | ❌ |
| **Fatal** | any | any | any | 运行时状态不可信，请重启 | 重启 | ❌ |

`Fatal` 与 `Degraded` 都禁用内容提交，输入可继续保留与复制；不自动清空草稿，不强制退出。`Fatal` 只提供查看最后接纳内容与重启提示；同一故障 store 不再承担读取、重载、隔离或保存。

---

## 6. 调度、发布与输入保护

### 6.1 单队列、候选闭包与入队边界

```rust
enum Command {
    Capture { request_id: RequestId, raw: String, draft_version: u64, ack: oneshot::Sender<Ack> },
    Complete { request_id: RequestId, id: SlipId, ack: oneshot::Sender<Ack> },
    ToggleCollapse { request_id: RequestId, id: SlipId, ack: oneshot::Sender<Ack> },
    Reload { request_id: RequestId, ack: oneshot::Sender<Ack> },
    IsolateAndReset { request_id: RequestId, ack: oneshot::Sender<Ack> },
    GetSnapshot { reply: oneshot::Sender<Result<UiSnapshot, QueryError>> },  // 可承载查询错误
    ViewEvent(ViewEvent),
    FlushPrefs,
    Shutdown { reply: oneshot::Sender<()> },
}
enum ViewEvent {
    MonitorsChanged(MonitorSnapshot),
    DragEnd { id: SlipId, order: u32 },
    DockSideChanged(DockSide),
}
```

app 只有一个消费者拥有 `presenter`。所有同步磁盘调用均 `spawn_blocking` 并等待结果；正常 store 结果依次执行"调用 → accept → 尝试发布 → 应答"，**不能把发布另开一个乱序任务**。

**候选闭包（四步）**：

1. 在 `LoadedDoc` 副本上按序应用 mutations，任一失败丢弃副本；
2. 对受影响的 `Slip` 跑 `validate_slip`；
3. 由**原始行数组局部替换**构造候选字节（禁止全文重序列化），用同一 parser 回读，断言：
   - 预期记录相等、ID 唯一；
   - **每个未修改行的输出字节 == 该行原始字节**（含终止符、未知片段、非法 UTF-8 普通行）；
4. 全部通过后才比较磁盘 hash、备份、提交。

`Noop` 不进入序列化/磁盘阶段。

**入队边界**：用户命令经有界队列 `try_send` 入队；`Full` → `rejected`/`Busy`，`Closed` → `rejected`/`NotExecuted`，均明确**尚未执行**。成功入队后，即使等待者超时或断开，也继续处理。队列容量是实现常量，首次取 64，实测后调整。`GetSnapshot` 入队失败通过 reply 返回 `QueryError`（或由调用方在 `try_send` 失败时直接返回，因此 reply 仍在调用方手里）。

平台事件与内部 `FlushPrefs` 不同步阻塞 OS/UI 回调：异步等待入队，退出时允许丢弃并记日志。消费者**不能向自己的有界队列等待发送**；偏好计时器只发无状态的 `FlushPrefs`，或由消费者自己的定时分支处理。

### 6.2 业务错误、Fatal 与正常退出

| 事件 | 当前请求 | 后续行为 |
|---|---|---|
| store 正常返回 `Domain`/`Conflict`/`Blocking`/`Io` 等 | 明确 `rejected`/`conflict`/`needsReload`，携带诊断和 `ui_rev` | accept 正常快照，继续队列 |
| 内容任务 `JoinError`、panic，或无法确认提交点的运行时故障 | `unknown`，不能伪造"未提交" | `runtime=Fatal`，冻结最后已接纳内容，**不再访问故障 store** |
| Fatal 下尚未执行或新到的内容命令 | `rejected`/`NotExecuted` | 不做磁盘操作；`GetSnapshot` 仍返回冻结内容和 Fatal 状态 |
| 接收方已断开，`reply.send` 失败 | 无法保证送达 | 日志记录，不撤销命令，不使消费者退出 |

**禁止在 `JoinError` 分支直接调用 `store.snapshot()` 补救**；panic 可能发生在提交后，并可能使 `Mutex` 中毒，立即重读不构成可靠恢复。也不以 `clear_poison` 后继续写作 v1 恢复策略。

只有带应答句柄的命令才要求一次发送尝试；`GetSnapshot`/`Shutdown` 使用各自的 reply 类型；`ViewEvent`/`FlushPrefs` **没有 ACK**。应答"恰好一次尝试"不等于网络/窗口一定收到。

**正常退出**：队列位置在 `Shutdown` 之前的命令顺序执行完；处理 `Shutdown` 时关闭入队端，后续缓冲命令逐个回 `NotExecuted`；在 `Running` 且偏好存储未故障时对 `dirty` prefs 最后尝试保存一次，再完成退出。等待当前已启动的阻塞任务结束，不能通过关窗或取消 `JoinHandle` 假装取消已开始的保存。

`Fatal` 不自动重启、不强退。当前进程保留只读 `GetSnapshot` 与草稿复制能力，其余 store/prefs 操作全部拒绝；用户决定退出后才结束消费者。**进程强制终止、abort/OOM 无法保证发出最终 ACK**，不包含在"必须应答"承诺内。

上述只读 Fatal 服务以消费者仍可运行为前提。app 保留消费者 `JoinHandle`；若消费者本身异常退出，关闭新命令入口并尽力经 shell 提示重启，不假称还能应答全部缓冲请求，不尝试重建故障 store。

### 6.3 presenter、通知失败与版本

```rust
pub struct Presenter {
    latest: StoreSnapshot,
    view: PaperViewSet,
    runtime: RuntimeStatus,
    current: Option<UiSnapshot>,
    ui_rev: u64,
}
```

初始化先载入一次内容与偏好，再创建 `ui_rev = 1` 的完整快照，随后开始接收命令。初始化 I/O 在阻塞环境完成，不在 UI 线程直接执行。

**`ui_rev` 递增规则（v11 明确）**：

- **凡生成并发射一份新的 `UiSnapshot`，无论来源（store 结果、视图事件、运行时变化），均递增 `ui_rev`**；
- `GetSnapshot` 返回已保存的 `current`，**不递增**；
- 传输重发沿用原版本；
- 若无此规则，`ViewEvent`（DPI、拖拽、dock_side，均无 ACK）生成的新几何会与上一份同版本，被前端 `s.ui_rev > last.ui_rev` **永久丢弃**。

`accept` 正常 outcome 时只作 `content_rev` 不回退的断言，无条件接纳，再由 `latest + view + runtime` 合成并保存 `current`，递增 `ui_rev` 后尝试 emit。视图变化同样从这些当前状态合成。

`emit` 失败只记录诊断，**不回滚 `latest`**，不把已提交操作改成失败；最终 ACK 引用已生成快照的 `ui_rev`。`GetSnapshot` 返回当前完整快照，供 §2.3 补偿读取。

`content_rev` 不能判断几何一定没变：DPI、显示器、折叠可独立改变布局。前端是否动画依据当前几何/展示差异；`content_rev` 仅提示内容是否重新接纳。

### 6.4 捕获窗口与输入不等待磁盘队列

全局快捷键回调经 shell 在平台要求的 UI/event-loop 上执行窗口 `show/focus`；它**不经过磁盘命令队列**。

窗口显示后的本地打字、IME、候选框与粗校验在前端处理；只有提交进入队列。前端不能自行替代原生窗口唤出与焦点切换。**捕获框的打开/关闭状态不进入 `UiSnapshot`**（§1.1），`presenter` 不感知。

慢磁盘期间视图命令可能排队，这项代价须在实测中记录；不为此立即拆双队列。

### 6.5 ACK、草稿与清空条件

```ts
type Ack = {
  requestId: string;
  result: "applied" | "rejected" | "conflict" | "needsReload" | "unknown";
  ui_rev?: number;   // 已生成可信 UI 状态时提供；传输失败可能没有
  error?: { code: string; message: string };
};
```

**`StoreError` → `Ack` 映射**（实现不得自行发挥）：

| 来源 | `result` | `error.code` |
|---|---|---|
| 成功 | `applied` | — |
| `Conflict` | `conflict` | `conflict` |
| `NeedsReload` | `needsReload` | `needsReload` |
| `ReadOnly`（Degraded） | `rejected` | `readOnly` |
| `Domain` | `rejected` | `domain` |
| `Blocking` | `rejected` | `blocking` |
| `Io` | `rejected` | `io` |
| `NotDegraded` | `rejected` | `notDegraded` |
| 队列 `Full` | `rejected` | `busy` |
| 队列 `Closed` / Fatal | `rejected` | `notExecuted` |
| `JoinError`/panic | `unknown` | `fatal` |

`requestId` 由前端传入（capture）或由 bridge 生成（其他命令）；仅关联应答，**不去重、不落盘**。`draftVersion` 是输入每次编辑的计数，留在请求关联信息中，不进入领域内容。

**`NeedsReload` 的第二个来源（v11 实施期补明确）**：`Patch` 指向一个当前文档里不存在的 `id`（用户对着旧快照点了"完成"，而那条已被外部删掉）也返回 `NeedsReload`。

- 这是**视图陈旧**，不是 ID 非法：该 id 是完全合法的 UUIDv7。把它映射成 `Domain(InvalidId)` 会让 `error.message` 输出 "invalid slip id"，对用户和日志都是假消息，也违反 `DomainError` "只表达领域规则不满足" 的契约。
- 该分支**不置** `needs_reload`：文件本身没有变化，正确的动作是让前端重新拉取快照后重试，而不是冻结后续写入。
- 因此 `needsReload` 的语义应读作"**调用方需要重读**"，而不只是"store 自己需要重载"。两者的前端动作相同（`GetSnapshot` 后重试），所以共用一个 code 是正确的。

**清空条件（四条件，v11 修明确）**：

```ts
applied
&& ack.requestId === inFlight.requestId
&& currentDraft.version === inFlight.draftVersion
&& ack.ui_rev != null && local.ui_rev >= ack.ui_rev   // 本地已看见
```

前三条防止"失败/超时/新草稿"被误清；**第四条防止"已保存但界面没刷新"时清空输入**——否则用户看到空输入框 + 旧界面，会确信碎片丢了。补偿失败时保留草稿，配「已保存，但界面同步失败，点此刷新」提示与可复制文本。

匹配判断引用该请求的**不可变提交记录**，不能拿后来请求的 `inFlight` 代替。

提交等待期间禁止同一草稿重复提交；允许继续编辑。超时只改变显示状态，**不取消原 Promise 或已入队工作**；迟到的真实 ACK 仍按四条件处理。`unknown` 保留输入、不自动重试。结果始终无法确认时，用户可复制草稿或显式手动重提；界面说明手动重提可能重复，**不能永久禁用而没有出口**。

### 6.6 超时与重载

`try_send` 成功才算入队。入队失败明确未执行；成功入队后的超时、窗口断开与应答丢失**不能推断未提交**。

重载只显示随后读到的文件状态，供人工核对。`requestId` 不落盘、快照无请求关联，外部编辑还可能删改已提交记录，因此**不能精确回答某个历史请求是否提交过**。

不自动重试 capture，不把重载当请求状态查询，不把未知结果变成普通业务失败。需要精确请求查询/幂等时，另按明确产品需求设计；v1 不加入持久请求日志。

---

## 7. 不变量与保证边界

| # | 不变量 | 验证方式 | 边界 |
|---|---|---|---|
| I1 | 内容与偏好磁盘写只经 `slip-store` | clippy、依赖检查、review | 不能约束外部编辑器或所有间接 I/O |
| I2 | `Frame` 由 `presenter` 管理；`shell` 只做平台映射 | 私有构造、review | 不把几何写进 `Slip` |
| I3 | `dock` 只吃快照与纯值 | 编译依赖与测试 | 无平台 API |
| I4 | 内容、展示、运行时诊断分别归属 | 类型与测试 | `Prefs`、`Fatal` 不进 Markdown |
| I5 | 领域值往返一致；未改行按字节保留 | 样本与往返测试 | BOM、追加分隔符、修改行规范化是显式例外 |
| I6 | 解析失败不自动覆盖主文件 | mtime/字节对比 | 用户明确恢复操作除外 |
| I7 | 前端不直接写文件，只经 bridge | capability 配置与扫描 | 自定义命令仍需 review |
| I8 | `Changed` 提交前比较原始字节 hash | 外部修改样本 | 不消除检查后到 rename 前的竞态 |
| I9 | 一次捕获最多一次主文件替换、rev +1 | 批量提交测试 | 备份替换不计为主文件提交 |
| I10 | IME 组合确认不触发提交 | 前端与真机 | 多输入法实测 |
| I11 | Tauri 调用集中在 bridge/shell 规定边界 | 扫描与 review | 扫描只是提醒 |
| I12 | 内容 store 内部写串行 | 并发测试 | 不承担外部进程互斥 |
| I13 | 候选可由同一 parser 回读，**且未修改行字节不变** | 格式化/序列化/解析闭包 + 字节断言 | 不恢复跨字段时间约束 |
| I14 | 已知提交后可处理失败不变成 `rejected` | 同步/通知失败注入 | 无可信返回的 panic 为 `unknown` |
| I15 | UI 显示最后观察到的写入门禁 | health/needs_reload/runtime 联合测试 | 无 watcher 时不预测未知磁盘变化 |
| I16 | hash 包括磁盘全部原始字节 | BOM/CRLF/非 UTF-8 普通行样本 | 不对 trim/转义后的值求 hash |
| I17 | 正常 store 结果接纳与执行同序 | `执行 A→accept A→执行 B→accept B` 轨迹 | 不用 `content_rev` 排序健康状态 |
| I18 | 清输入须 **applied + requestId + draftVersion + 本地已收敛** | 前端测试 | 迟到 ACK 同样检查 |
| I19 | 首订阅无漏更新、无回退；**视图事件也递增 `ui_rev`** | 订阅/查询交错 + DPI 事件测试 | 订阅必须 await |
| I20 | 健康文件不被重复隔离；**无效文件可再隔离** | 恢复矩阵测试 | 外部保存仍有竞态边界 |
| I21 | 超时不取消已入队请求 | 慢 store 与断开接收方 | 强制终止不保证应答 |
| I22 | 合法性不依赖时钟先后 | 回拨、外部勾选样本 | 时间仍需可按 RFC3339 写出 |
| I23 | panic 不被伪造成未提交并继续写 | 提交前后任务故障测试 | Fatal 不再调用故障 store |
| I24 | 未完成条目不因残留归档时间隐藏 | 展示与重新完成测试 | 读取时不改写原始字段 |
| I25 | ACK 关联的快照事件丢失可补取 | emit 失败后 ACK/ui_rev/getSnapshot | 补取失败需显示同步提示且**不清空输入** |
| I26 | 捕获框状态不进 `UiSnapshot`、不进队列 | 类型与 review | 前端本地状态 |

---

## 8. 静态检查

1. `fmt` / `clippy -D warnings` / `test`
2. clippy：`disallowed-types = ["std::fs::File","std::fs::OpenOptions"]`；`disallowed-methods = ["std::fs::write","std::fs::remove_file","std::fs::rename"]`；`slip-store` 内 allow，其余 deny。
3. 扫描（提醒，非保证）：`std::fs` 只在 `slip-store`；`dock.rs` 无平台 API；`shell.rs` 外无 `unsafe`；`invoke(` 只在 `bridge.ts`；**候选文档构造处不存在全文重序列化**（review + 闭包字节断言）。

---

## 9. 验收

### 9.1 必须覆盖的回归

1. **格式闭包**：时间回拨仍可写可读；RFC3339 无法格式化的时间和非 v7 ID 在写盘前拒绝，主文件不变。
2. **异常元数据可见性**：`done=false` + `archived_at` 存在仍显示；完成后不自动归档；无 `done_at` 的手工完成仍可读。
3. **恢复矩阵**：`Normal`+`ReadWrite` 返回 `NotDegraded`；`Degraded` 外部修复被真正接纳；`Isolated`+`NotFound` 能创建；**`Isolated`+现有文件无效 → 可再次隔离并重建（v11 新增出口）**；`AlreadyExists` 不覆盖。
4. **任务异常**：模拟提交前/后的任务故障，当前请求均 `unknown`、`runtime=Fatal`；后续写操作未执行，且不调用故障 `store.snapshot`。
5. **通知补偿**：内容已提交但 emit 失败，ACK 仍 `applied`；前端依 `ui_rev` 补取快照，最终与 presenter 一致，不重提 mutation。
6. **顺序与退出**：断言执行/接纳交替轨迹；接收方断开不取消已入队工作；正常 `Shutdown` 不遗留尚在运行的保存。
7. **清空四条件（v11 新增）**：`applied` + emit 失败 + `getSnapshot` 也失败 → **输入保留**且有"已保存"提示；本地已收敛到 `ack.ui_rev` 时才清空。
8. **视图事件版本（v11 新增）**：`ui_rev=21` 生成后 emit 失败，随后一次 DPI 事件 → 最终 `ui_rev>21` 且几何、内容均为最新。

### 9.2 其他验收样本

- **文本往返**：`&`、`<`、`&amp;`、`&lt;`、HTML 注释片段、反斜杠、`C:\temp`、中文、emoji、Unicode 首尾空白、CRLF/LF/CR 混合、无末尾换行、空文件、仅 BOM 文件。
- **原字节保留**：未修改行、未知 metadata 顺序和重复项、非法 UTF-8 普通行；**候选构造后逐行比对未修改行字节**；非法 UTF-8 记录候选必须 `Degraded`。
- **追加**：空文档一次追加 3 条；已有文件无终止符；文件首个分隔符与多数分隔符不同；新末行继承 EOF 属性。
- **文法**：`extra=a;b` 为 `InvalidRecord`；`extra=a;b=c` 为两个合法未知 kv；中文未知值按 `value_char` 规则处理；已知键重复拒绝；`[X]` 可读、重写后为 `[x]`；键顺序不影响结果。
- **外部编辑**：只勾选、取消勾选但残留 `done_at`、未完成但残留 `archived_at` 均可读，读取不改文件。
- **Noop**：重复完成和设置同一正文不写盘、不备份、不增 `content_rev`；不能借 `Noop` 声称已检查外部变化。
- **事务**：同批先成功后失败不改文件或已载入内容；重复 ID 拒绝；成功后回读得到全部预期记录。
- **备份**：同毫秒冲突不覆盖；成功轮转保留 5 代；失败不影响主提交。
- **提交点**：备份失败和提交后目录同步失败不改变已提交结果；临时文件写入/同步/替换失败明确未提交。
- **冲突**：hash 不同不写任何文件；`needs_reload` 期间不读写；有效重载清除，失败重载仍发布门禁。
- **偏好**：旧 `FlushPrefs` 不覆盖更新的折叠/排序；损坏隔离失败不阻断内容；正常退出最后尝试保存 dirty 状态。
- **前端**：`rejected`/`conflict`/`needsReload`/`unknown` 均不清输入；新草稿不被旧 ACK 清空；首订阅与查询交错无回退；迟到成功 ACK 与当前草稿正确关联。
- **队列**：`Full`/`Closed` 明确尚未执行；内部消息无 ACK 的行为符合类型；Fatal 仍可查看冻结快照和复制草稿。
- **状态组合**：§5 表格五种组合各自文案与主操作正确，尤其是 `Degraded`+`needs_reload` 不显示"外部已修改"。

### 9.3 端到端延迟

T0 = 全局快捷键回调入口；T1 = 输入框可接收首字符；20 预热 + 300 样本；p95 < 150ms（开发期阈值）；附环境清单。

### 9.4 IME

三家输入法各 100 字：中英混输、退格、选词、粘贴，**"选词按 Enter 不得提交"**。

### 9.5 内存

private bytes，进程树合计；基线静置 5 分钟；24h；< 20MB 且无单调上升。

### 9.6 规模与启动测量

1000 / 5000 / 20000 条记录分别记录：

- `T_read_parse`：Rust 读取、解析、构造首份 `StoreSnapshot`；1000 条 < 30ms 是待实测目标；
- `T_ui_ready`：进程启动到首屏可交互，单独计入 Tauri/WebView 初始化，**不与纯 parse 混为同一指标**；
- `T_commit`：候选复制、序列化回读、hash、备份、sync、替换及队列等待，记录 p50/p95。

冷/热缓存分别测量，附文件大小、正文长度分布、机器、文件系统、构建模式与依赖版本。只有确认 parse 成为瓶颈后才依 §10 评估索引，**不能用 WebView 启动慢作为加索引的证据**。

### 9.7 窗口 spike（决定 §2.4 是否成立）

1. 空白区点击穿透到下层应用；
2. 纸片区可点击、可输入、焦点不丢；
3. 中文候选框跟随纸片；
4. 捕获框关闭后焦点回到原应用；
5. 跨两个不同 DPI 的显示器：移动中与停下后命中都正确；
6. 纸片边缘、阴影区、快速进出的命中；
7. F11 最大化与独占全屏（视频播放器）分开验收。

任一持续抖动 → 切每纸一窗，并按 §2.4 列明的边界评估返工范围。

#### 9.7.1 本机实测记录（2026-11-10，`slip-shell` 启动自检输出）

已机械确认：

| 项 | 实测 |
|---|---|
| 窗口创建 | `label=main`；`420×560` 逻辑尺寸在 150% DPI 下为 `630×840` 物理像素 |
| `alwaysOnTop` | `is_always_on_top()` 读回 **`true`** —— 平台确实应用了，不是只写在配置里 |
| 无边框 / 透明 / 跳过任务栏 | 配置已生效，窗口无系统边框、无任务栏条目 |
| 启动焦点 | `focused=false`：启动**不抢**当前应用焦点（符合便签纸预期） |
| 显示器枚举 | 2 台，`\\.\DISPLAY1` 2560×1600 @1.5（主）、`\\.\DISPLAY5` 3840×2160 @1.5 |

**本机无法验证第 5 条"跨不同 DPI"**：两台显示器缩放都是 150%。必须在存在
100% 或 125% 显示器（或临时改一台）的机器上补测，否则这一条只算未验。

**自动截图不能作为透明与否的证据**：DirectComposition 窗口的透明区在
`BitBlt`/`CopyFromScreen` 截图上会渲染成近黑，无法区分"真透明"与"透明失效"。
把窗口拖到深色与浅色壁纸交界处，人眼一两秒即可判定。

仍需人眼确认的清单：

1. 空白区是否真的透出桌面并让点击落到下层应用；
2. 纸片区可输入、焦点不丢、中文候选框跟随移动；
3. 调用 `spike_hide` 后 1.2 秒内原应用是否拿回焦点；
4. 纸片边缘 / 阴影区 / 快速进出的命中；
5. F11 最大化与独占全屏视频分别验收。

外壳已提供对应按钮（穿透开关、置顶开关、读窗口状态、列显示器、隐藏/显示），
不需要改代码就能跑完这 5 条。

---

## 10. 触发条件

| 现在不做 | 触发信号 |
|---|---|
| SQLite 索引 | 需要搜索；或 §9.6 实测 1000 条超 30ms 且影响体验 |
| watcher / 自动重载 | 用户抱怨"改完不刷新"且手动重载高频 |
| 三方合并 | 引入 watcher 后 `Conflict` 仍频繁 |
| CLI / MCP 命令转发 | 明确需要；先定义"已收到/已落盘/失败"三态应答 |
| Specta 类型生成 | TS 与 Rust 边界类型首次真实漂移 |
| bridge 第二实现 | 需要自动化 UI 验证 |
| 每纸一窗 | §9.7 spike 失败 |
| 拆 crate | 第二消费者，或测试/编译成本可测拖累 |
| 事件/通知机制 | ≥3 个 post-commit 消费者且需异步/取消/超时 |
| 物理归档 | 单文件 >1MB 且影响启动 |

---

## 11. 禁入清单

DI 容器 / 通用状态管理库 / ORM / 可注册回调 / 事件总线 / 实时 watcher / 按内容去重 / 物理归档 / daemon / 组件直接 `invoke()` / `shell.rs` 外 `unsafe` / `slip-store` 依赖 `tauri` / v1 的 SQLite 索引 / **把跨字段语义或时钟顺序当作文件合法性判据** / **候选文档全文重序列化**。

---

## 12. 实施顺序与 M0/M1/M2 切分

### M0：主链路可跑（先做到能 `cargo test` + 手动跑通）

1. **domain**：`normalize_text`、`validate_slip`（格式闭包）、状态转换、拆条；
2. **store/doc**：文法、记录候选、原字节保存、metadata 更新、批量追加、**局部替换构造 + 回读字节断言**；
3. **store 事务**：锁、候选校验、hash、备份、sync、提交点、`Conflict`、`needs_reload`、`ReadWrite`/`Degraded` 两态；
4. **最小调度**：单队列、`accept`、`UiSnapshot`、首屏。

M0 验收子集：§9.1 第 1、6 条；§9.2 中"文本往返""原字节保留""追加""文法""事务""提交点""冲突"。

### M1：恢复与偏好

5. 恢复矩阵：隔离、重建、外部修复、`Isolated` 死锁出口（§9.1 第 3 条）；
6. 偏好：单一拥有者、`FlushPrefs`、原子替换、损坏降级。

### M2：异常路径与前端保护

7. `Fatal`、通知补偿、ACK `ui_rev` 补取、队列 `Full`/`Closed`（§9.1 第 4、5、7、8 条）；
8. bridge/前端：首订阅、四条件清空、IME、状态组合文案。

### 并行

- **窗口 spike** 与 M0 并行推进；结论决定 `shell` 与 §2.4 的成立与否，**不延后到完整 UI 做完再判断**。

### 停止条件

除发现**可复现的契约矛盾**或**真实平台失败**，不再为假设增加架构层；以实现与测量结果驱动下一轮修订。本轮的 4 条必修均属可复现契约矛盾（每条都有对应测试），不是新增假设。

---

## 13. 平台依据与修订边界

以下链接是技术约束依据，不代表 Slip 已完成相应实现或实测。具体构建以锁定依赖版本为准。

- [R1：time 的 RFC3339 格式化实现](https://docs.rs/time/latest/src/time/formatting/formattable.rs.html)：验证实际格式边界，不能仅检查 `OffsetDateTime` 类型是否成立。
- [R2：Tokio spawn_blocking](https://docs.rs/tokio/latest/tokio/task/fn.spawn_blocking.html)：阻塞任务启动后不能靠 abort 取消；超时与退出需单独定义。
- [R3：Rust Mutex poisoning](https://doc.rust-lang.org/std/sync/struct.Mutex.html#poisoning)：panic 后不应假设锁内状态仍可直接继续使用。
- [R4：tempfile NamedTempFile::persist](https://docs.rs/tempfile/latest/tempfile/struct.NamedTempFile.html#method.persist)：原子替换与内容、目录同步是不同事项。
- [R5：Tauri Event API](https://v2.tauri.app/reference/javascript/api/namespaceevent/)：订阅建立是异步操作。
- [R6：Tauri AppHandle::run_on_main_thread](https://docs.rs/tauri/latest/tauri/struct.AppHandle.html#method.run_on_main_thread)：已在文档中确认存在，语义为"在主线线程运行给定闭包"；shell 负责平台事件循环上的原生窗口操作。

---

## 14. 实施状态（M0 + M1 已完成，垂直链路打通）

本节记录**代码实际到什么程度**，与上面的**设计意图**分开看。所有数字来自一次干净的 `cargo test --workspace --offline`。

### 14.1 已落地

| crate | 内容 | 测试 |
|---|---|---|
| `slip-domain` | `normalize_text`、`validate_slip`/`validate_time` 格式闭包、`apply_patch` 状态转换、`split_drafts` 拆条、`SlipId` 严格解析 | 51 |
| `slip-store` · `doc` | `parse`/`render`/`apply`/`verify_roundtrip`；3 种终止符、BOM、转义、未知 kv 保序、`extra=a;b` 唯一结果 `BadKeyValue` | 37（含在 80 内） |
| `slip-store` · 事务 | `MarkdownStore`：锁覆盖全事务、候选校验、BLAKE3 hash、备份、`sync_all`、提交点、`Conflict`、两态 health、恢复矩阵 A/B + `Isolated` 死锁出口、偏好原子替换 | 80 |
| `slip-store` · 验收 | 真实临时目录 + 真实文件：格式闭包、时钟回拨、元数据可见性、冲突不落字节、恢复、陈旧视图、提交点 | 12 |
| `slip-app` | `Presenter`（合成 + `ui_rev` 全事件递增）、`Dispatcher`（单队列单消费者、`spawn_blocking`、错误映射、`Fatal`、`Full`/`Closed`/`Shutdown` drain） | 20 |
| `slip-app` · 流水线 | 真实磁盘 + 真实 store + 真实 dispatcher：捕获落盘并回读、重启后读回、多行拆条、领域拒绝不碰文件 | 4 |
| `src-tauri` + `ui` | 窗口外壳、7 个 IPC 命令、Svelte 5 前端（捕获框、列表、勾选、状态条、spike 面板） | — |

**合计 167 个测试通过，0 个 `#[ignore]`**；`cargo clippy --workspace --all-targets -- -D warnings` 与 `cargo fmt --all --check` 均干净。仓库无任何 `todo!()` / `unimplemented!()`。

M0 验收子集（§9.1 第 1、6 条；§9.2 的文本往返、原字节保留、追加、文法、事务、提交点、冲突）**已由 `crates/slip-store/tests/acceptance.rs` 覆盖**，且刻意不使用 mock——验证的就是"磁盘上到底发生了什么"。

### 14.1.1 垂直链路已用真实界面验证

不是"看着应该行"，而是通过 WebView2 的 CDP 端点驱动真实窗口跑出来的：

| 驱动动作 | 实测结果 |
|---|---|
| 启动 | 空 vault → `meta = 0 条未完成 · rev 1`，显示空态文案，`window.__TAURI__` 存在 |
| 输入 + Enter | 提示变 `已收录`，列表出现该条，输入框清空，`rev 1 → 2` |
| 磁盘 | `- [ ] 从 CDP 写入的一条 <!-- slip:v1;id=01a1c48c-…;created=2026-11-10T08:21:25.8Z -->`，无多余换行，备份已生成 |
| 输入法组合态按 Enter | 输入**保留**（`半截拼音`），未提交，条数不变 |
| 勾选 | 该条变 `done`，`0 条未完成`，`rev 2 → 3`，磁盘变 `- [x] ` 且写入 `;done_at=` |
| 取消勾选 | 回到未完成，`rev 3 → 4`，磁盘回到 `- [ ] ` |

这张表同时钉住了 §6.6 的四条件清空与 §7.x 的 IME 门控——它们此前只是设计文字。

### 14.2 实施期的三处契约补明确

1. **`write_atomic` 的 `Err` 严格只表示"未提交"**；目录同步拆成独立的 `fsops::sync_dir()`，在 `rename` 成功之后调用。这样 §4.4 的"替换成功后目录 sync 失败 → 已提交 + `DurabilityUnconfirmed`"才能落地，而不需要"提交后返回 `Err`"这种自相矛盾的签名。Windows 上 `sync_dir` 恒 `Ok`（不伪造失败事件）。
2. **`ApplyError::SlipNotFound` → `NeedsReload`**，见 §6.5 的补充说明。
3. **`RoundtripError` 无对应 `StoreError` 变体**：映射为 `Io(IoKind::Other)` + `tracing::error` + 诊断，且**绝不写盘**。它是防 `doc.rs` 回归的机械保证，正常路径下不应出现。

### 14.3 未完成

- **捕获框独立窗口**：现在唤出的是主窗口本身。设计上捕获框应当是独立小窗，
  否则"记一句话"会连带展开整个 Inbox。
- **§9.7 人工验收 5 条**：见 §9.7.1，其中"跨不同 DPI"本机无 100%/125% 显示器，不可验。
- **偏好持久化未接前端**：`prefs.json` 只被读取，折叠/排序还没产生写入。
- **`ui_rev` 事件推送**：当前靠每次 ACK 后 `GetSnapshot` 补偿，未用事件通道；
  补偿路径是契约允许的，但事件能省一次往返。
- **热键不可配置**：写死 `Ctrl+Alt+Space`，冲突时用户没有退路。
- **启动可见性未与产品语义对齐**：见 §14.6 的记录，是个待定的产品决定。

### 14.5 M2：热键、托盘、单实例、自启（已完成并验证）

新增 3 个文件、改动 2 个：

| 文件 | 职责 |
|---|---|
| `src-tauri/src/hotkey.rs` | 全局快捷键注册、`summon`/`dismiss`、`dismiss_window` 命令 |
| `src-tauri/src/system.rs` | 托盘菜单（显示 / 打开 Inbox / 开机自启 / 退出）、tooltip、打开文件 |
| `crates/slip-store/tests/perf.rs` | §9.3–9.6 的测量（4 个 `#[ignore]`） |
| `ui/src/App.svelte`、`Capture.svelte` | 监听 `slip://summon`、Esc 收走、捕获成功后自动收窗 |

插件：`tauri-plugin-global-shortcut`、`tauri-plugin-autostart`、`tauri-plugin-single-instance`
（单实例**必须最先注册**，否则其它插件可能在它判定前就建好窗口）。`tauri` 开启 `tray-icon` feature。

Capability 只加了实际用到的：`core:window:allow-hide/show/set-focus`、`core:event:default`、
`global-shortcut:allow-register/unregister`、`autostart:allow-enable/disable/is-enabled`。

**真实热键验收**（Win32 `keybd_event` 发真键 + CDP 读界面 + 文件系统读磁盘 + `GetForegroundWindow` 读焦点）：

| 阶段 | 实测 |
|---|---|
| 发键前前台 | Windows Terminal |
| 发键后前台 | `[Slip] class=Tauri Window` —— 热键真的唤出了窗口 |
| 界面焦点 | `focusedTag=TEXTAREA`、`focusedClass=capture__input` |
| 输入 + 回车 | `hint=已收录`，列表出现该条，输入框清空 |
| 磁盘 | `- [ ] 热键唤出后写入的一条 <!-- slip:v1;id=01a1c4a1-… -->` |
| 收走后前台 | 回到 Windows Terminal —— **原应用拿回了焦点**（§9.7 第 3 条） |

### 14.5.1 §9.3–9.6 性能实测

`crates/slip-store/tests/perf.rs`，真实临时目录 + 真实 fsync，四种构建配置各跑一遍。
运行方式：`cargo test --release -p slip-store --test perf --offline -- --ignored --nocapture`

| 指标 | debug | release |
|---|---|---|
| 启动解析 1000 行（新 store + snapshot） | 16.1 ms | 11.4 ms |
| 启动解析 10000 行 | 79.1 ms | 22.5 ms |
| 单条追加 p50 / p95（1000 条之上） | 15.8 / 18.5 ms | 14.8 / 17.9 ms |
| 勾选 p50 / p95 | 15.3 / 18.5 ms | 15.4 / 19.1 ms |
| 10000 条文件体积 | 1074.2 KB（110 B/条） | 同 |

**两个数字指向不同结论，这是这组测量最值钱的地方：**

1. **解析是 CPU 受限**：release 比 debug 快 3.5 倍（79 → 22.5 ms）。10000 行 22 ms 完全可接受。
2. **写入不是 CPU 受限**：debug 与 release 几乎相同（15.8 vs 14.8 ms）。说明这 15 ms
   花在同步 I/O 上——整文件重写 110 KB + 备份 + `sync_all` + 目录同步。
   **优化代码不会让它变快，只有改提交策略才会。**

推论：
- 单次交互 15 ms 无感，且批量录入是一次事务、一次写入，不会线性叠加。
- 但整文件重写意味着**写入耗时随文件长度线性增长**：10000 条（1.1 MB）时单次追加
  约是 1000 条的十倍量级。这是"Markdown 为真源 + 无索引"的固有代价，
  用户应当定期归档。**这是设计上的已知取舍，不是缺陷**。

### 14.6 产品形态：召唤式（已决定并落地）

**决定：召唤式。** 窗口默认隐藏，托盘是唯一的常驻入口；热键唤出，记完或 Esc 收走。

理由：原始需求是"轻量、低摩擦地记录琐碎思考"。常驻便签会让纸片变成需要管理的桌面物件，
而召唤式把成本压到"一个热键 + 一句话"。

落地要点：

| 项 | 做法 |
|---|---|
| 启动可见性 | `tauri.conf.json` 的 `visible: false` |
| 双击 exe 不能"什么也没发生" | 自启插件带 `--autostart` 标记参数；**只有**带该参数才静默，手动启动就直接唤出 |
| 托盘是唯一入口 | tooltip 与菜单都显示**实际生效**的热键（不是写死的字符串） |
| 热键可配 | `ui-state.json` 的 `hotkey` 字段；解析失败回落默认并留 warn |

#### 14.6.1 这个决定暴露的一个数据丢失 bug

把 `hotkey` 加进 `Prefs` 之后，`PaperViewSet::to_prefs()` 立刻编译失败——它只搬运
`order` / `collapsed` / `dock_side` 三个字段。而 `dispatch.rs` 里任何一次拖拽或折叠
触发的偏好保存，走的正是 `presenter.view().to_prefs()`。

也就是说：**用户改了热键，随便拖动一张纸片，热键配置就被静默擦回默认值。**

修法是让 `from_prefs` / `to_prefs` 恢复成无损往返——`PaperViewSet` 原样携带
`hotkey` 但**从不解释它**。已加回归测试 `view_roundtrip_never_drops_the_hotkey`。

教训：**当一个结构体是"整体读、整体写"的载体时，只搬运它关心的字段就是在慢慢丢数据。**

### 14.7 实现期新增的两个测试缝（生产无旁路）

为了在 `doc.rs` 尚是 `todo!()` 时能测事务层，引入了：

- `DocOps`：解析/应用/渲染/回读四个函数指针。类型与字段均为**私有**，生产唯一构造路径是 `DocOps::real()`，外部无法注入替身。
- `fsops::failpoints`：**整个模块 `#[cfg(test)]`**，用于注入备份失败、目录同步失败、固定时钟。

两者都不进入生产构建行为，也不放宽任何公开可见性；`tests/acceptance.rs` 只使用公开 API。
