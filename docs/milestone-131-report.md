# R82–R131 里程碑报告

> 汇总第二轮 50 轮迭代。**所有数字都能用文末命令重算**；这不是脚本生成的，是人工汇总。
> 上一轮（R32–R81）的报告在 `docs/final-50-rounds-report.md`。

## 一、总体

| 项 | 数值 |
|---|---|
| 提交总数 | **153**（标题含轮次编号的 97 个） |
| Rust 测试（Linux/WSL） | **282 项** |
| 前端纯逻辑断言 | **268 条** |
| 变异审计 | **10 处核心不变量**，10/10 可被测试抓住 |
| IPC 命令 | **26** 个 |
| 代码规模 | Rust 11,578 行 / 前端 4,623 行 / 工具脚本 3,944 行 / 文档 3,964 行 |
| 产物 | CSS 11.73 KB（预算 14） / JS 81.55 KB（预算 88） |

工具脚本（3,944 行）与前端代码（4,623 行）仍处在同一量级——**门禁本身是需要维护的产物**。

## 二、这一轮最重要的决定：先审计，再动手

动手前用只读子智能体把 R82–R131 矩阵与既有不变量对撞了一遍（`docs/audit-r82-r131.md`），
结果是 **6 项硬冲突**：

| 轮次 | 冲突 | 处置 |
|---|---|---|
| R99 | "锁定条目绝对保护"与锁的语义相反（锁只拦正文） | 纠偏：保护正文，完成/归档照常 |
| R102 | 整文件 `compact()` = 全文重序列化 + 不可逆删除 | **改方案 A**：只读容量统计 |
| R103 | 依附 R102 | 只读命令 `vault_capacity` |
| R104 | "不可撤销"却挂撤销条 = 死按钮 | 单条删除 + 双确认 + **不挂撤销** |
| R107 | 内存暂存 = 第二真源 | 草稿留前端 + 明确文案 |
| R108 | lossy 载入 = 静默改写用户字节 | 保留隔离 + **只读预览**（`read_corrupt_preview`） |

另外 6 轮（R82/R88/R120/R121/R123/R126）被判定"已有实现"，直接跳过以免刷版本。

**这一步的价值高于任何单个功能**：把"凭空造出来的需求"拦在写进代码之前。

## 三、已落地的新能力

| 能力 | 轮次 | 关键约束 |
|---|---|---|
| `#tag` 零正则解析 | R83 | 无 regex 依赖；URL 锚点天然排除；**派生视图只读** |
| 标签徽章 / 过滤语法 `#tag` `is:` | R84–R86 | 不合法 `is:xxx` 忽略而非按字面搜 |
| 中英混排间距 / 超长串折行 / 搜索高亮 | R87/R89/R90 | 只做渲染层，绝不改写正文 |
| 置顶（`pinned`） | R92–R95 | marker 写 `pinned=1`；与 `locked` 同类，**不进撤销栈** |
| 批量完成 / 归档双确认 | R96/R97 | 批 = 单事务单撤销步 |
| 草稿清空与单次恢复 | R98 | 只在空时恢复，不吞新输入 |
| 撤销连击排队 | R100 | 整体替换类操作必须清栈 |
| 只读容量统计 | R102A | 口径 = `done && archived_at` |
| 备份混合保留 | R105 | 至少留一代"更早那天" |
| 损坏文件只读预览 | R106 | lossy 只用于预览，绝不写回 |
| 单条彻底移除 | R104 | 三道保护；成功后清撤销栈 |
| 导出命名 `export-YYYYMMDD-HHmmss.md` | R110 | 同秒撞名加 `-2`，永不覆盖 |
| 托盘三动作 / 热键备选 | R115/R116 | 零新依赖、零新 capability |
| 混合 DPI 核对 / 12px 安全边 | R112/R113 | `CURSOR_GAP` 语义未动 |
| 触控右滑 / 收起淡出 | R114/R119 | 尊重 `prefers-reduced-motion` |

## 四、工程强化

| 项 | 轮次 | 说明 |
|---|---|---|
| 50,000 行压测 | R122 | debug 293 ms / release 29.7 ms（门限 1000/150） |
| 1000 条渲染与内存 | R124 | 8,021 节点（8.0/条）；20 次重建 **0 增长** |
| 写入中断原子性 | R111 | 新 failpoint：临时文件写好但未替换时失败 → 字节/rev/记录数三者不变 |
| 常量检查 | R128 | 死常量 + 前端不许出现配额字面量 |
| 变异审计扩到 10 处 | R129 | 并因此**发现一个真缺口**（见下） |
| 发布包 SHA-256 | R127 | 清单用 `sha256sum -c` 可验格式 |
| CI Linux 逻辑层 job | R125 | 先改政策表述，再动 workflow |

## 五、这一轮抓到的真缺陷

1. **`ondblclick` 位置错误**（我 R46 引入）：归档/恢复/锁定三颗按钮的 `ondblclick`
   落在 `</svg>` 之后，成了按钮内的**文本节点**——三颗按钮的双击阻断从未生效。
2. **「置顶不进撤销栈」没有守卫**（R129 变异审计发现）：把 `SetPinned` 加进 `undoable`
   白名单后，全套测试仍然全绿。补了两条断言。
3. **`purge` 用错校验方式**：复用了 `verify`（行数不变的闭合检查），删除一行必然失败，
   错误只显示 `Io(Other)`。改成"重新解析 + 比对记录集合"。
4. **导出断言的假阳性**：`contains("-2")` 在文件名 `export-20261112-…` 上恒为真
   （那个 `-2` 来自年份）。改成精确比对文件名。

## 六、已知欠账（诚实清单）

| 项 | 状态 | 说明 |
|---|---|---|
| 端到端 `verify-hotkey.ps1` | **未在最近几轮复跑** | exe 时而被 SAC 拦；最后一次完整全绿是 R64 的 121 项 |
| release 构建 / 正式发布包 | **仍未成功** | release 侧 proc-macro DLL 被 SAC 拒；打包链路本身已用 debug 产物验证 |
| R67 `skipTaskbar` | **未定论（已裁决为接受）** | 判据（扩展样式）不可靠，改为实测值输出 + 人工目视 |
| 归档抽屉容量摘要的实时性 | 有意为之 | 只在打开抽屉与删除后刷新（省体积）；抽屉开着时归档会短暂过期 |
| Windows 侧 `cargo test --workspace` | 本机跑不了 | `slip_shell-*.exe` 被 SAC 拒；WSL 侧 282 项全绿、clippy 全目标可编译 |
| 发丝边框对比度 1.31 / 1.43 | **有意低于 3:1** | 装饰性元素；交互线索（焦点环 4.91/4.94、次要文字 4.58/5.48）已合规并单独断言 |
| CSS / JS 预算 | **两次提额** | CSS 10→14 KB、JS 70→80→88 KB；注释里写明"再提额前先问是不是该拆模块或砍功能" |

## 七、方法学结论（新增）

1. **审计先于实现**：把"会破坏不变量的需求"拦在动手之前，收益远大于多做一个功能。
2. **变异测试会暴露"只是注释的承诺"**：10 处里 1 处无守卫，恰是最容易被当成"显然成立"的那条。
3. **工具的失败形态也是产品**：变异脚本崩溃时留下了改坏的源码，因此加了 `atexit` 全局兜底；
   常量检查器漏算测试文件导致误报，因此把"测试也是消费者"写进它的注释。
4. **误报会杀死守卫**：三条新检查都在上线前做了误报抑制与正反验证。
5. **预算数字要能追溯**：两次提额都写明了触发事件、实测数字与"这不是无限放宽"的边界。

## 八、怎么复现

```powershell
git rev-list --count HEAD
git log --oneline -200 | Select-String 'R[0-9]{2,3}'

wsl.exe -d Arch -- bash -lc 'sh /mnt/d/Dev/simple-todo/tools/wsl-test.sh'
& "C:\Users\spand\.bun\bin\bun.exe" run tools/check-ui-logic.mjs
python tools/mutation-audit.py
pwsh -NoProfile -File tools/check-dead-constants.ps1
pwsh -NoProfile -File tools/check-doc-sync.ps1
pwsh -NoProfile -File tools/check-bundle-size.ps1

# 50,000 行解析（release）
wsl.exe -d Arch -- bash -lc 'cd /mnt/d/Dev/simple-todo && cargo test --release --target-dir $HOME/.cache/slip-target --offline -p slip-store --test perf -- --ignored --nocapture startup_parse_survives'

# 1000 条渲染与内存（需要能启动的 exe + CDP）
$env:SLIP_VAULT = "<含 1000 条合法记录的 vault>"
$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = "--remote-debugging-port=9223"
Start-Process .\target\debug\slip-shell.exe
& "C:\Users\spand\.bun\bin\bun.exe" tools/cdp-memory.mjs 9223 1000
```

## 九、最终核查（收尾时实测）

```
WSL 全套              282 项通过
前端纯逻辑断言         268 条通过
cargo clippy          --workspace --all-targets -D warnings 通过
cargo fmt --check     通过
不可回退约束（8 项）    通过
设计文档同步           通过（IPC 26 个、capability 2 项）
钩子与 CI 零漂移       通过（各 10 个关口）
常量检查（死代码+镜像）  通过
变异审计              10 处，10/10 可被测试抓住
产物                  CSS 11.73 KB / 14 KB、JS 81.55 KB / 88 KB
```

**Windows 侧 `cargo test --workspace` 在本机跑不了**：`slip_shell-*.exe` 测试二进制被
Smart App Control 拒（连试 4 次 + 删掉重建 3 次均被拒）。这是**环境**问题，不是代码问题——
`clippy --all-targets` 能把包括测试在内的全部目标编译过，而同样的测试在 WSL 里 282 项全绿。
