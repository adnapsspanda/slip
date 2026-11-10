# Rust + Tauri 开源项目架构调研

> 面向 Slip v2 的架构参考，供开发者与 AI coding 工具协作时共同使用。
> 调研日期：2026-11-10。本文基于公开仓库与 Tauri 官方文档；项目状态可能变化，采纳前请查看链接中的最新实现。

## 结论摘要

Slip v2 的方向与成熟 Tauri 项目的常见边界一致：Rust 管理文件与应用状态，Svelte 负责展示，Tauri commands 作为明确的 IPC 边界。当前建议保留 `domain`、`store`、`app` 三个 crate，并将 `dock`、`shell`、`presenter` 暂留在 app 内部。优先跑通 vertical slice，再按真实需求拆分。

从成熟项目借鉴边界原则，不复制其规模。Slip 目前不需要 daemon、通用操作注册表、插件系统或大型 monorepo 工具链。

## 调研范围与判断口径

调研覆盖 GitButler、Spacedrive、Kunkun，以及更接近 Slip 产品形态的 doc-md 和 Sticky-Note，并参考 Tauri 官方架构与安全文档。项目成熟度并不相同：GitButler、Spacedrive 可用于观察较成熟或规模化的结构；doc-md 与 Sticky-Note 主要用于对照产品设计，不能仅凭公开页面推断其用户规模或长期稳定性。

## 项目观察

### GitButler：Rust 后端、Svelte 前端、共享核心

GitButler 使用 Tauri、Svelte/TypeScript 和 Rust。其开发文档将磁盘操作放在 Rust，将界面放在前端；Rust 后端也服务于 CLI。其前端把 Tauri 接触点集中在 backend 接口与实现中，并通过 Web 实现支持非 Tauri 场景。

**对 Slip 的借鉴：**保持命令处理薄；集中前端 IPC 调用；若未来需要浏览器 mock 或替换壳层，再为 bridge 添加第二种实现。第一版不必建立通用 RPC 或庞大的 backend 抽象。

来源：[GitButler README](https://github.com/gitbutlerapp/gitbutler)、[开发文档](https://github.com/gitbutlerapp/gitbutler/blob/master/DEVELOPMENT.md)、[迁移评估中的 IBackend 设计](https://github.com/gitbutlerapp/gitbutler/blob/master/electron-migration-assessment.md)

### Spacedrive：领域与操作分开，类型可跨语言共享

Spacedrive 将领域数据与操作分层，并区分状态变更操作和查询；桌面端经 TypeScript 客户端连接 Rust 后端，并由 Rust 类型生成 TypeScript 类型。它还采用 daemon 与多客户端架构。

**对 Slip 的借鉴：**命令/查询的输入输出应显式、稳定，Rust 类型适合成为 IPC 契约来源。daemon、JSON-RPC、操作注册表和多客户端体系面向更复杂的共享核心；Slip 单 GUI 写入者阶段不需要这些。

来源：[Spacedrive 架构与开发说明](https://github.com/spacedriveapp/spacedrive/blob/main/AGENTS.md)

### Kunkun：Svelte + Tauri 的跨平台启动器

Kunkun 是 Tauri 与 Svelte/SvelteKit 应用，使用 pnpm 与 Cargo workspace，并包含扩展系统。

**对 Slip 的借鉴：**可参考其 Svelte 与 Rust 的协作方式。Slip 暂时没有第三方扩展需求，不应提前引入扩展 API、权限系统或 monorepo 复杂度。

来源：[Kunkun 仓库](https://github.com/kunkunsh/kunkun)、[项目结构说明](https://github.com/kunkunsh/kunkun/blob/develop/QWEN.md)

### doc-md：Markdown 本地优先应用

doc-md 使用 Tauri、Rust 与 Svelte，围绕本地 Markdown 文件工作。其公开架构将文件访问、watcher 与窗口管理放在 Rust，将部分解析、索引与搜索放在前端。

**对 Slip 的借鉴：**重视 Markdown 的可读、可编辑和外部变更检测；同时认识到前后端职责可以有不同切法。Slip 若坚持 Rust 是状态权威，就不应直接复制其前端处理边界。

来源：[doc-md 仓库与架构说明](https://github.com/paperhurts/doc-md)

### Sticky-Note：与 Slip 相近的桌面便签场景

该项目展示托盘、全局快捷键、置顶便签、多窗口、单实例和独立 note-store crate 等实现方向。

**对 Slip 的借鉴：**可作为窗口与托盘行为清单的参考；公开用户信号有限，因此不把它视为成熟度或成功度的主要证据。

来源：[Sticky-Note 仓库](https://github.com/ikohnt/Sticky-Note)

### Cap：技术路线会随产品变化

Cap 的仓库当前说明，发布版桌面应用已从 Tauri 转向 GPUI，Tauri 版本保留作开发 harness。这提醒我们：选择 Tauri 本身不是目标，窗口、媒体或原生交互需求才决定技术路线。

**对 Slip 的借鉴：**通过 IME、热键、透明窗口和性能 spike 验证 Tauri 是否满足实际需求，不因其他项目采用 Tauri 就跳过验证。

来源：[Cap 当前仓库结构与发布说明](https://github.com/CapSoftware/Cap/blob/main/AGENTS.md)

## 对 Slip v2 的建议

### 建议保留

- `slip-domain`：Slip 内容模型与纯状态转换。
- `slip-store`：Markdown 读写、索引、备份、外部变化处理。
- `slip-app`：Tauri commands、启动顺序、快照组装。
- `dock`、`shell`、`presenter` 暂留 app 内部模块，待出现第二消费者或平台扩展压力后再拆 crate。
- Rust 持有内容状态权威；Svelte 接收快照、发出 Intent。

### 建议补上的薄边界

在前端建立单一 bridge 模块，例如：

```ts
appendSlip(text)
completeSlip(id)
toggleCollapse(id)
subscribeSnapshot(listener)
```

Svelte 组件通过这些函数与 Rust 通信，避免组件到处直接调用 `invoke()`。先保持实现简单；只有在需要浏览器 mock、自动化测试或替换桌面壳时，再添加第二种 bridge 实现。

Rust commands 应做输入校验、调用 app/store 并返回稳定结果，不承载文件格式、布局算法等业务实现。`Intent`、`UiSnapshot`、`Health` 等边界类型保持明确；接口稳定后，再考虑 Specta 一类的 Rust → TypeScript 类型生成，避免在领域模型仍频繁变化时增加生成步骤。

### Tauri 权限

按窗口配置最小 capabilities。不要向 UI 暴露通用文件写能力；内容写入仍由 Rust store 完成。Capabilities 限制 WebView 对 Tauri commands 和插件的访问，不会自动约束 Rust 内部代码，因此“只有 store 写文件”仍需源码检查、测试与 review 配合。

来源：[Tauri 架构](https://tauri.app/concept/architecture/)、[Capabilities](https://v2.tauri.app/security/capabilities/)、[Runtime Authority 与安全边界](https://v2.tauri.app/security/runtime-authority/)

## 暂不引入

- **Daemon、多客户端 IPC、JSON-RPC：**等 CLI、后台服务或其他独立写入端成为明确需求后再做。
- **通用事件总线：**当前捕获、持久化和 UI 更新可由显式函数与状态发布完成。
- **插件系统：**没有首个真实扩展需求前，不承担插件 API、权限隔离和版本兼容成本。
- **更多 crate 与大型 monorepo：**只有模块边界稳定、存在第二消费者或编译/维护成本可测时再拆分。

## 建议的实现顺序

1. 验证 Tauri 窗口、IME 候选框、热键和 DPI 行为。
2. 实现“热键 → 中文输入 → 写入 `inbox.md` → 桌面显示 → 勾选完成”的 vertical slice。
3. 固化 Rust command 与 UI snapshot 契约，并集中前端 bridge。
4. 增加 Markdown 外部变更、索引重建和降级恢复路径。
5. 依据真实的第二消费者、跨平台或扩展需求，决定是否类型生成、拆 crate 或引入额外 IPC。

## 给 AI coding 工具的协作约定

- 开始改动前先读本文件和仓库根目录 `AGENTS.md`。
- 领域模型、存储语义或 command 契约变更时，说明受影响的层与不变量。
- 不因“未来可能需要”提前加入 daemon、事件总线、插件系统或额外 crate。
- 不把 `Slip` 内容状态与 PaperView 展示状态、Frame 瞬时几何状态混在一起。
- 写文件的职责保持在 store；前端只发 Intent 并消费快照。
- 对架构约束说明它属于类型系统、测试、CI 扫描还是人工 review，不夸大其保证范围。

## 参考资料

- [Tauri 官方架构](https://tauri.app/concept/architecture/)
- [Tauri Capabilities](https://v2.tauri.app/security/capabilities/)
- [GitButler](https://github.com/gitbutlerapp/gitbutler)
- [Spacedrive](https://github.com/spacedriveapp/spacedrive)
- [Kunkun](https://github.com/kunkunsh/kunkun)
- [doc-md](https://github.com/paperhurts/doc-md)
- [Sticky-Note](https://github.com/ikohnt/Sticky-Note)
- [Cap](https://github.com/CapSoftware/Cap)
