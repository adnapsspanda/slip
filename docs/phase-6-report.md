# 阶段六：数据容灾、备份健康与外部交互（R52–R61）

> 本阶段开始碰 Rust 存储层，验证条件比前两阶段好：**大部分测试能在 WSL 里真跑**，
> 不受本机 Smart App Control 影响（`tools/wsl-test.sh`）。

## 一、逐轮结果

| 轮次 | 内容 | 结论 |
|---|---|---|
| R52 | 「隔离并重建」两步确认 | 已实现（4 秒窗口，超窗/时钟回拨退回重武装） |
| R53 | 备份名解析收紧 | 已修（多余段、路径分隔符） |
| R54 | 只读排查指引 | 已实现（`<details>` 折叠四步，无新 capability） |
| R55 | 导出按平台原生换行 | 已实现（领域层可测，无 BOM） |
| R56 | 损坏文件定位 | **原样已满足**，只补证据 |
| R57 | 诊断补显示器/缩放/平台 | 已实现（build 号有意不做，见偏差说明） |
| R58 | 写盘重试指数退避 | 已实现（20→320ms，总 620ms） |
| R59 | 空文件是健康 vault | 补测试钉住（原本无测试） |
| R60 | 版本号溢出统一饱和 | 已修（`content_rev` 原为裸 `+1`，release 下会回绕到 0） |
| R61 | 存储与容灾综合验收 | 本文件 |

## 二、这一阶段修掉的四个真实缺陷

1. **R60 `content_rev` 会回绕到 0**：裸 `+1` 在 release 下溢出回绕，而 0 的语义是
   "尚未载入"——等于把冲突检测的依据倒退回初始态。`ui_rev` 那边却是饱和的，
   两者不一致本身就是隐患。
2. **R53 畸形备份名可被当成备份恢复**：`parse_backup_name` 忽略多余段、且不校验
   路径分隔符，`a/b.bak-1` 也能解析通过。
3. **R58 固定 40ms 退避抗不住同步盘**：OneDrive 持锁常超过 160ms，退避用尽就把 vault
   降级成只读，而用户看不出原因。
4. **R52 一次点击就清空重建**：没有二次确认，手滑即为数据丢失。

## 三、验证证据（WSL 真跑）

```
工具：wsl.exe -d Arch -- sh tools/wsl-test.sh
```

| 测试目标 | 结果 |
|---|---|
| `slip-domain` lib | **66 通过** |
| `slip-domain` frontend_contract | 1 通过 |
| `slip-store` lib | **103 通过** |
| `slip-store` acceptance | 12 通过 |
| `slip-store` archive_guard | 1 通过 |
| `slip-store` format_closure | 7 通过 |
| `slip-app` lib | 17 通过 |
| `slip-app` pipeline | 8 通过 |
| 合计 | **215 通过、0 失败**（4 项 perf 按 `#[ignore]` 跳过） |

### 数据安全相关的测试清单（逐个点名）

**提交点与原子性**
- `fsops::write_atomic_replaces_content_and_leaves_no_temp_files`
- `fsops::sync_dir_failure_is_observable_after_commit`
- `tests::commit_ack_survives_backup_failure`
- `tests::commit_ack_survives_directory_sync_failure`

**冲突与外部修改**
- `tests::external_edit_produces_conflict_without_writing`
- `hash_covers_bom_crlf_and_non_record_bytes`

**恢复与隔离**
- `tests::degraded_invalid_file_is_isolated_then_rebuilt`
- `tests::invalid_file_isolated_with_real_parser`
- `tests::isolate_requires_degraded`
- `tests::isolated_with_existing_valid_file_accepts_it`
- `tests::isolated_with_invalid_file_reisolates_and_rebuilds`
- `tests::isolated_with_missing_file_creates_empty`
- `fsops::isolate_steps_over_preexisting_evidence`
- `fsops::isolate_never_overwrites_same_millisecond`

**备份**
- `fsops::backup_captures_content_at_call_time`
- `fsops::backup_never_overwrites_same_millisecond`
- `fsops::list_backups_is_newest_first_and_ignores_other_files`
- `fsops::prune_backups_keeps_newest_generations`
- `fsops::restore_backup_overwrites_with_the_chosen_generation`
- `fsops::restore_backup_refuses_names_outside_the_directory`
- `fsops::parse_backup_name_rejects_malformed_names`（本轮新增）
- `fsops::malformed_backup_names_are_ignored_and_not_restorable`（本轮新增）

**BOM 往返**
- `doc::bom_is_stripped_from_view_and_restored`
- `doc::commit_strips_bom_but_noop_does_not_change_it`

**版本号**
- `tests::content_rev_strictly_increases_and_saturates_at_the_ceiling`（本轮新增）

## 四、没跑到的那部分（不混进上表）

| 项 | 原因 |
|---|---|
| Windows 侧 `cargo test -p slip-store` | 测试二进制被 SAC 拒绝（3 次重试全拒） |
| `verify-hotkey.ps1` 端到端 | 同上（exe 无法启动） |
| R57 诊断新增字段的运行时输出 | 同上，只做了构建与 clippy 验证 |

## 五、与矩阵措辞的偏差（两处，都有理由）

1. **R53** 矩阵要求"非标准命名的文件标记为 corrupt"。本项目里 corrupt 专指
   `inbox.md.corrupt-*`（真实数据丢失的证据）。把名字不合规的备份也标成 corrupt 会污染
   这个概念，所以选择**忽略**——既不当备份，也不进 corrupt 列表。
2. **R57** 矩阵要求"操作系统构建版本"。取真实 build 号需要原生 API（`RtlGetVersion`）或
   引入 `windows` crate 直连依赖；为一条诊断信息改依赖图，风险（新 build script 被 SAC 拒、
   依赖漂移）大于收益。本轮只报 `os / arch`。

## 六、一个反复出现的操作教训

往 Rust 测试模块里插测试，我**连续四轮踩到同一类错**：

1. R53：插在 `#[test]` 之后 → 属性被挤走，`duplicated attribute`。
2. R58：插在 `#[cfg(windows)]` 之后 → 属性错位，Linux 编不过。
3. R59：同 R53，但 **WSL 居然编过了**，只有 Windows 侧 clippy 报错。
4. R60：直接插进 `impl` 块 → `the test attribute may only be used on a free function`。

根因是**用脚本按文本锚点改 Rust 源码**。修正后的纪律：

- 插入测试前，先读目标位置前后 5 行确认结构（是不是属性块、是不是 impl 内部）；
- 插入后**两边都跑**（WSL `cargo test` + Windows `cargo clippy --all-targets`）——
  只跑一边会漏，第 3 条就是活例。

## 七、下一阶段

进入**第四阶段：多屏边缘、多实例与外壳窗口契合（R62–R71）**。这批在 `src-tauri`
与 `hotkey.rs`，测试受 SAC 影响较大，但纯数学部分（窗口定位）可以抽成纯函数在 WSL 验证。
