//! Slip 的 Tauri 外壳。
//!
//! 它只做三件事，且刻意保持薄：
//! 1. 把窗口配成"无边框置顶便签纸"（见 `tauri.conf.json`）；
//! 2. 解析 vault 路径、构造 store/presenter/dispatcher，交给 Tauri 托管；
//! 3. 把 `Dispatcher` 的四个命令暴露成 IPC。
//!
//! **任何业务判断都不在这里**：拆条在 `slip-domain`，落盘在 `slip-store`，
//! 顺序与状态在 `slip-app`。外壳只做类型搬运，所以它可以被替换而不动契约。

mod diag;
mod hotkey;
mod platform;
mod system;

use std::path::PathBuf;
use std::sync::Arc;

use slip_app::dispatch::Dispatcher;
use slip_app::presenter::{PaperViewSet, Presenter};
use slip_app::{Ack, AckError, AckResult, UiSnapshot};
use slip_domain::{Mutation, Patch, Slip, SlipId, split_drafts};
use slip_store::prefs::{JsonPrefsStore, PrefsStore};
use slip_store::{MarkdownStore, Store};
use tauri::Manager;
use time::OffsetDateTime;

/// 托管进 Tauri 的运行时状态。
pub struct ShellState {
    dispatcher: Dispatcher,
    /// **必须持有**：消费者的任务跑在这个运行时上，drop 掉就等于杀掉调度器。
    _runtime: tokio::runtime::Runtime,
    pub(crate) vault: PathBuf,
    /// 实际生效的热键（已解析、失败时已回落默认值）。
    ///
    /// 托盘 tooltip、菜单和日志都用它，保证「显示给用户的」与「真正注册的」
    /// 是同一个字符串——否则用户看到的热键会骗人。
    ///
    /// 为什么需要内部可变性（R116）：托盘里可以「改用 Win+Alt+S」并**重试注册**，
    /// 成功后就地更新这一格。锁的只是一个字符串，**不是** presenter——
    /// 与"单队列单消费者、presenter 不上锁"的约束无关。
    hotkey: std::sync::Mutex<String>,
}

impl ShellState {
    /// 当前**实际注册成功**的热键文本；未注册时为空串。
    pub(crate) fn hotkey_text(&self) -> String {
        match self.hotkey.lock() {
            Ok(slot) => slot.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    /// 注册成功后就地更新。读失败（中毒）也照写：显示错误的代价比重试注册本身更大。
    pub(crate) fn set_hotkey(&self, value: String) {
        match self.hotkey.lock() {
            Ok(mut slot) => *slot = value,
            Err(poisoned) => *poisoned.into_inner() = value,
        }
    }
}

/// 开机自启时传给本进程的标记参数。
///
/// 窗口配置是 `visible: false`（召唤式），但那样用户双击 exe 会「什么也没发生」。
/// 用这个参数区分：开机拉起 → 静默驻留托盘；手动双击 → 直接把窗口显示出来。
const AUTOSTART_FLAG: &str = slip_app::launch::AUTOSTART_FLAG;

/// 热键状态（IPC 载荷）。
#[derive(serde::Serialize)]
pub struct HotkeyStatus {
    /// 美化后的热键文本；未注册时为空。
    pub text: String,
    /// 是否注册成功。
    pub registered: bool,
}

/// 解析 vault 目录。
///
/// 用可见路径而不是隐藏的 appdata：Markdown 是真源，用户随时会想用编辑器打开它。
/// `SLIP_VAULT` 可覆盖，用于测试与多套并行实例。
fn resolve_vault() -> PathBuf {
    if let Some(custom) = std::env::var_os("SLIP_VAULT") {
        return PathBuf::from(custom);
    }
    let home = platform::home_dir().unwrap_or_else(|| PathBuf::from("."));
    let documents = home.join("Documents");
    platform::vault_from(&home, documents.is_dir())
}

fn rejected(request_id: &str, code: &str, message: impl Into<String>) -> Ack {
    Ack {
        request_id: request_id.to_owned(),
        result: AckResult::Rejected,
        ui_rev: None,
        changed: false,
        error: Some(AckError {
            code: code.to_owned(),
            message: message.into(),
        }),
    }
}

// ---------------------------------------------------------------- 命令

/// 读当前快照。这是发射丢失后的**补偿路径**，不递增 `ui_rev`。
#[tauri::command]
async fn get_snapshot(state: tauri::State<'_, ShellState>) -> Result<UiSnapshot, String> {
    state
        .dispatcher
        .get_snapshot()
        .await
        .map_err(|error| error.0)
}

/// 把输入框内容拆成若干纸条并全部追加。
///
/// 拆条与校验都在领域层：这里只负责"生成 id / 时间"这一件外壳该干的事。
#[tauri::command]
async fn capture(
    state: tauri::State<'_, ShellState>,
    request_id: String,
    text: String,
) -> Result<Ack, String> {
    let now = OffsetDateTime::now_utc();
    let drafts = match split_drafts(&text, now) {
        Ok(drafts) => drafts,
        Err(error) => {
            return Ok(rejected(&request_id, "domain", error.to_string()));
        }
    };
    let mutations: Vec<Mutation> = drafts
        .into_iter()
        .map(|draft| {
            Mutation::Append(Slip {
                id: SlipId::new_v7(),
                text: draft.text,
                done: false,
                created_at: draft.occurred_at,
                done_at: None,
                archived_at: None,
                // 新收录的纸条永远不是定稿状态。
                locked: false,
                // 新收录的纸条永远不置顶（R92 领域字段，外壳只负责给初值）。
                pinned: false,
            })
        })
        .collect();
    Ok(state.dispatcher.apply(request_id, mutations).await)
}

/// 勾选 / 取消勾选一条。
#[tauri::command]
async fn complete(
    state: tauri::State<'_, ShellState>,
    request_id: String,
    id: String,
    done: bool,
) -> Result<Ack, String> {
    let id = match SlipId::parse_strict(&id) {
        Ok(id) => id,
        Err(error) => {
            return Ok(rejected(&request_id, "domain", error.to_string()));
        }
    };
    let patch = Patch::SetDone {
        done,
        at: OffsetDateTime::now_utc(),
    };
    Ok(state
        .dispatcher
        .apply(request_id, vec![Mutation::Patch { id, patch }])
        .await)
}

/// vault 里遗留的损坏文件——**只返回文件名，不返回绝对路径**。
///
/// 界面只需要"有几个、叫什么"，绝对路径既没必要暴露给定 webview 的代码，
/// 也是 `reveal_corrupt` 拒绝接收前端路径的前提。
#[tauri::command]
async fn corrupt_files(state: tauri::State<'_, ShellState>) -> Result<Vec<String>, String> {
    let vault = state.vault.clone();
    tauri::async_runtime::spawn_blocking(move || system::corrupt_artifacts(&vault))
        .await
        .map_err(|error| format!("读取损坏文件被中断：{error}"))
        .map(|paths| {
            paths
                .into_iter()
                .filter_map(|path| path.file_name().map(|n| n.to_string_lossy().into_owned()))
                .collect()
        })
}

/// 在资源管理器里选中最新的损坏文件。定位目标由 Rust 自己查，不接受参数。
#[tauri::command]
fn reveal_corrupt(app: tauri::AppHandle) -> Result<(), String> {
    system::reveal_newest_corrupt(&app)
}

/// 用系统默认浏览器打开正文里的链接。
///
/// 校验全在 Rust 侧：前端只负责把用户写的原文递过来，**它不参与判断**
/// "这个 URL 能不能开"——那种判断放在能被注入的地方等于没做。
#[tauri::command]
fn open_url(url: String) -> Result<(), String> {
    system::open_url(&url)
}

#[tauri::command]
async fn reload(state: tauri::State<'_, ShellState>, request_id: String) -> Result<Ack, String> {
    Ok(state.dispatcher.reload(request_id).await)
}

/// 改写一条纸条的正文。
///
/// 已定稿（`locked`）的纸条会在这里被领域层拒绝——外壳不做二次判断，
/// 否则"锁"就有了两个实现，迟早会不一致。
#[tauri::command]
async fn edit(
    state: tauri::State<'_, ShellState>,
    request_id: String,
    id: String,
    text: String,
) -> Result<Ack, String> {
    let id = match SlipId::parse_strict(&id) {
        Ok(id) => id,
        Err(error) => {
            return Ok(rejected(&request_id, "domain", error.to_string()));
        }
    };
    let patch = Patch::SetText(text);
    Ok(state
        .dispatcher
        .apply(request_id, vec![Mutation::Patch { id, patch }])
        .await)
}

/// 切换定稿标记。锁定与解锁都必须能点，否则锁是单向门。
#[tauri::command]
async fn set_locked(
    state: tauri::State<'_, ShellState>,
    request_id: String,
    id: String,
    locked: bool,
) -> Result<Ack, String> {
    let id = match SlipId::parse_strict(&id) {
        Ok(id) => id,
        Err(error) => {
            return Ok(rejected(&request_id, "domain", error.to_string()));
        }
    };
    let patch = Patch::SetLocked(locked);
    Ok(state
        .dispatcher
        .apply(request_id, vec![Mutation::Patch { id, patch }])
        .await)
}

/// 切换置顶。与 `set_locked` 同构：一条 Patch、一次 `apply`、`content_rev` 只 +1。
///
/// 为什么与锁定分开成两条命令而不是合成 `set_flag(id, flag, value)`：
/// 合成会让"置顶"和"锁定"共用一个错误面，将来任何一方的规则变化都会牵连另一方。
/// 两条窄命令更容易守住各自的语义。
#[tauri::command]
async fn set_pinned(
    state: tauri::State<'_, ShellState>,
    request_id: String,
    id: String,
    pinned: bool,
) -> Result<Ack, String> {
    let id = match SlipId::parse_strict(&id) {
        Ok(id) => id,
        Err(error) => {
            return Ok(rejected(&request_id, "domain", error.to_string()));
        }
    };
    let patch = Patch::SetPinned(pinned);
    Ok(state
        .dispatcher
        .apply(request_id, vec![Mutation::Patch { id, patch }])
        .await)
}

/// 批量切换完成状态（R96 的"完成匹配项"）。
///
/// 一批走**同一个 `apply` 批次**：共用一次原子替换、`content_rev` 只 +1，
/// 且在撤销栈里算作一步——按一次 `Ctrl+Z` 整批恢复。这与 `archive` 的批处理语义一致。
///
/// 与单条 `complete` 并存而不是替换它：单条是热路径（空格键），批量是低频操作，
/// 合并成一个命令会让热路径多一次 `Vec` 分配。
#[tauri::command]
async fn complete_many(
    state: tauri::State<'_, ShellState>,
    request_id: String,
    ids: Vec<String>,
    done: bool,
) -> Result<Ack, String> {
    let at = OffsetDateTime::now_utc();
    let mut mutations = Vec::with_capacity(ids.len());
    for raw_id in ids {
        let id = match SlipId::parse_strict(&raw_id) {
            Ok(id) => id,
            Err(error) => {
                return Ok(rejected(&request_id, "domain", error.to_string()));
            }
        };
        mutations.push(Mutation::Patch {
            id,
            patch: Patch::SetDone {
                done,
                at: at.replace_nanosecond(0).unwrap_or(at),
            },
        });
    }
    if mutations.is_empty() {
        return Ok(rejected(
            &request_id,
            "domain",
            "没有可操作的纸条".to_owned(),
        ));
    }
    Ok(state.dispatcher.apply(request_id, mutations).await)
}

/// 归档一条或多条已完成的纸条。
///
/// 一批归档走同一个 `apply` 批次：共用一次原子替换、`content_rev` 只 +1，
/// 且在撤销栈里算作一步，按一次 `Ctrl+Z` 即可整批恢复。
#[tauri::command]
async fn archive(
    state: tauri::State<'_, ShellState>,
    request_id: String,
    ids: Vec<String>,
) -> Result<Ack, String> {
    let at = OffsetDateTime::now_utc();
    let mut mutations = Vec::with_capacity(ids.len());
    for raw_id in ids {
        let id = match SlipId::parse_strict(&raw_id) {
            Ok(id) => id,
            Err(error) => {
                return Ok(rejected(&request_id, "domain", error.to_string()));
            }
        };
        mutations.push(Mutation::Patch {
            id,
            patch: Patch::Archive { at },
        });
    }
    Ok(state.dispatcher.apply(request_id, mutations).await)
}

/// 把一条已归档的纸条恢复回列表。
///
/// 归档在界面上是"眼不见为净"，但 `archived_at` 是**墓碑**不是删除——恢复只是清掉它。
/// 没有这个入口，抽屉就只是个只能看不能动的死胡同。
#[tauri::command]
async fn unarchive(
    state: tauri::State<'_, ShellState>,
    request_id: String,
    id: String,
) -> Result<Ack, String> {
    let id = match SlipId::parse_strict(&id) {
        Ok(id) => id,
        Err(error) => {
            return Ok(rejected(&request_id, "domain", error.to_string()));
        }
    };
    let patch = Patch::Unarchive;
    Ok(state
        .dispatcher
        .apply(request_id, vec![Mutation::Patch { id, patch }])
        .await)
}

/// 把当前清单导出成一份不带机器标记的 Markdown，并在资源管理器里定位到它。
///
/// 内容取自**视图快照**而不是 `inbox.md`：后者带 marker，导出的东西是给人看的，
/// 不该把人造标记一起带走。返回文件路径，供界面显示。
#[tauri::command]
async fn export_markdown(state: tauri::State<'_, ShellState>) -> Result<String, String> {
    let snapshot = state
        .dispatcher
        .get_snapshot()
        .await
        .map_err(|error| format!("读取快照失败：{error:?}"))?;
    let rows: Vec<(&str, bool)> = snapshot
        .slips
        .iter()
        .filter(|slip| !slip.archived)
        .map(|slip| (slip.text.as_str(), slip.done))
        .collect();
    let title = "Slip 导出";
    let content = system::render_plain(title, &rows);
    let vault = state.vault.clone();
    let path = tauri::async_runtime::spawn_blocking(move || system::write_export(&vault, &content))
        .await
        .map_err(|error| format!("导出被中断：{error}"))??;
    // 导出完直接定位到文件：用户下一步多半就是复制或打开它。
    // 定位失败只影响便利性，不该让导出本身报错，所以只忽略。
    let _ = platform::reveal(&path);
    Ok(path.display().to_string())
}

/// 外部修改到底改了什么：只读解析磁盘，与当前视图比条数。
///
/// 只在 `needsReload` 为真时有意义——那是"磁盘比内存新"的唯一信号。
#[tauri::command]
async fn external_diff(state: tauri::State<'_, ShellState>) -> Result<system::DiffSummary, String> {
    let snapshot = state
        .dispatcher
        .get_snapshot()
        .await
        .map_err(|error| format!("读取快照失败：{error:?}"))?;
    let path = state.vault.join("inbox.md");
    // 整份解析放到阻塞线程：大 inbox（10k 条）或网络盘上，这个同步调用会卡住
    // async worker，而它只是给横幅算"外部改了几条"。
    let peeks = tauri::async_runtime::spawn_blocking(move || slip_store::peek_slips(&path))
        .await
        .map_err(|error| format!("读取 inbox 被中断：{error}"))?
        .map_err(|kind| format!("读取 inbox 失败：{kind}"))?;
    let disk: Vec<(String, String, bool)> = peeks
        .into_iter()
        .map(|slip| (slip.id.to_canonical_string(), slip.text, slip.done))
        .collect();

    let current: Vec<(String, String, bool)> = snapshot
        .slips
        .iter()
        .map(|slip| (slip.id.clone(), slip.text.clone(), slip.done))
        .collect();
    Ok(system::diff_summary(&current, &disk))
}

/// 列出 vault 里的备份（新的在前）。只给名字、大小与时间，不给完整路径。
///
/// 为什么是 `async` + `spawn_blocking`：这个命令要对**每一份**备份完整解析一遍
/// （`list_backups` 里逐份 `doc::parse`）。备份可能有十几代，同步命令会把这些
/// I/O 与解析全部压在主线程上，窗口正好在这时候卡住。vault 路径先克隆出来，
/// 闭包里不再碰 Tauri 托管的 `State`。
/// 返回 `Result` 而不是裸 `Vec`：`spawn_blocking` 的任务可能 panic 或被取消，
/// 那时"空列表"与"真的没有备份"无法区分——界面会显示一份虚假的"没有备份"。
/// 拒绝则走前端的错误分支，至少提示里说得清是"读取失败"。
#[tauri::command]
async fn backup_list(state: tauri::State<'_, ShellState>) -> Result<Vec<BackupRow>, String> {
    let vault = state.vault.clone();
    let list =
        tauri::async_runtime::spawn_blocking(move || slip_store::fsops::list_backups(&vault))
            .await
            .map_err(|error| format!("读取备份列表被中断：{error}"))?;
    Ok(list
        .into_iter()
        .map(|info| BackupRow {
            name: info.name,
            size: info.size,
            stamp_ms: info.stamp_ms,
            slips: info.slips,
            preview: info.preview,
        })
        .collect())
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct BackupRow {
    name: String,
    size: u64,
    stamp_ms: u64,
    /// 这一代里有几条记录——区分两代备份最要紧的信息。
    slips: usize,
    /// 第一条的摘要，给人一眼认出来的钩子。
    preview: String,
}

/// 用指定备份恢复。
///
/// 名字的合法性由**存储层**判定（裸文件名 + 必须是本目录下的备份），
/// 外壳不重复实现那套规则——两处各写一份只会漏。
#[tauri::command]
async fn restore_backup(
    state: tauri::State<'_, ShellState>,
    request_id: String,
    name: String,
) -> Result<Ack, String> {
    Ok(state.dispatcher.restore_backup(request_id, name).await)
}

/// 本次运行的热键状态。
///
/// 为什么单独一条命令、而不是塞进 `UiSnapshot`：`Presenter` 在热键注册**之前**就已创建，
/// 并且由消费者任务独占（单写者）。为一条"启动即定、运行期不变"的信息去改那条路径，
/// 换来的复杂度不值。这里读的是外壳状态，代价是一次内存读取。
#[tauri::command]
fn hotkey_status(state: tauri::State<'_, ShellState>) -> HotkeyStatus {
    let hotkey = state.hotkey_text();
    HotkeyStatus {
        text: system::pretty_hotkey(&hotkey),
        registered: !hotkey.is_empty(),
    }
}

/// 把诊断信息写进系统剪贴板，方便用户贴给他人排查（R08）。
///
/// 三条边界：
/// - **在 Rust 侧写剪贴板**，不给 webview 授剪贴板权限；
/// - 内容全部来自**已发布的快照**与外壳已知状态：不读磁盘、不读日志、不扫目录；
/// - **只写不读**：我们不从剪贴板取任何东西。
#[tauri::command]
async fn copy_diagnostics(
    app: tauri::AppHandle,
    state: tauri::State<'_, ShellState>,
) -> Result<usize, String> {
    use tauri_plugin_clipboard_manager::ClipboardExt;

    let snapshot = state
        .dispatcher
        .get_snapshot()
        .await
        .map_err(|error| format!("读取快照失败：{error:?}"))?;

    let mut text = String::from("Slip 诊断\n");
    text.push_str(&format!("vault: {}\n", state.vault.display()));
    // 显示器与缩放：窗口定位错乱、字太小/太大这类问题，没有这两个数字根本没法聊。
    // 取主显示器（列表第一项）；多屏时把每个都列出来。
    match app.primary_monitor() {
        Ok(Some(monitor)) => {
            let size = monitor.size();
            let scale = monitor.scale_factor();
            text.push_str(&format!(
                "display: {}x{} @ {scale}x\n",
                size.width, size.height
            ));
        }
        _ => text.push_str("display: <读取失败>\n"),
    }
    match app.available_monitors() {
        Ok(monitors) if monitors.len() > 1 => {
            let list: Vec<String> = monitors
                .iter()
                .map(|monitor| {
                    let size = monitor.size();
                    format!("{}x{}@{}x", size.width, size.height, monitor.scale_factor())
                })
                .collect();
            text.push_str(&format!("monitors: {}\n", list.join(", ")));
        }
        _ => {}
    }
    // 平台与架构：能直接给出，不需要任何额外依赖。
    text.push_str(&format!(
        "platform: {} / {}\n",
        std::env::consts::OS,
        std::env::consts::ARCH
    ));
    let registered = state.hotkey_text();
    let hotkey = if registered.is_empty() {
        "<未注册>"
    } else {
        registered.as_str()
    };
    text.push_str(&format!("hotkey: {hotkey}\n"));
    text.push_str(&format!(
        "content_rev: {}  ui_rev: {}\n",
        snapshot.content_rev, snapshot.ui_rev
    ));
    text.push_str(&format!("writable: {}\n", snapshot.health.writable));
    text.push_str(&format!("needs_reload: {}\n", snapshot.needs_reload));
    text.push_str(&format!("recovery: {:?}\n", snapshot.recovery));
    text.push_str(&format!("runtime: {:?}\n", snapshot.runtime));

    if snapshot.health.warnings.is_empty() {
        text.push_str("warnings: (无)\n");
    } else {
        // BTreeSet 没有 join，先收集再拼。
        let warnings: Vec<&str> = snapshot
            .health
            .warnings
            .iter()
            .map(String::as_str)
            .collect();
        text.push_str(&format!("warnings: {}\n", warnings.join(", ")));
    }

    if snapshot.diagnostics.is_empty() {
        text.push_str("diagnostics: (无)\n");
    } else {
        text.push_str("diagnostics:\n");
        for line in &snapshot.diagnostics {
            text.push_str(&format!("  - {line}\n"));
        }
    }

    let chars = text.chars().count();
    app.clipboard()
        .write_text(text)
        .map_err(|error| format!("写入剪贴板失败：{error}"))?;
    Ok(chars)
}

/// 回滚最近一次内容编辑、勾选或归档。
#[tauri::command]
async fn undo(state: tauri::State<'_, ShellState>, request_id: String) -> Result<Ack, String> {
    Ok(state.dispatcher.undo(request_id).await)
}

#[tauri::command]
async fn isolate_and_reset(
    state: tauri::State<'_, ShellState>,
    request_id: String,
) -> Result<Ack, String> {
    Ok(state.dispatcher.isolate_and_reset(request_id).await)
}

/// **彻底移除**一条已归档记录（R104 方案 A 的落地：单条删除，不是整文件 compact）。
///
/// 与 `archive` 的区别是"真的把那一行从文件里删掉"。因为不可逆：
/// - 后端只允许删**已归档**的记录（store 里还有一道同样的保护）；
/// - 与 `isolate_and_reset` 同属"整体替换类"操作，成功后必须清掉撤销栈——
///   否则撤销条会指向一条已经不存在的记录。
#[tauri::command]
async fn purge_archived(
    state: tauri::State<'_, ShellState>,
    request_id: String,
    id: String,
) -> Result<Ack, String> {
    let id = match SlipId::parse_strict(&id) {
        Ok(id) => id,
        Err(error) => {
            return Ok(rejected(&request_id, "domain", error.to_string()));
        }
    };
    // 走 dispatcher 是为了复用同一条队列与 ACK 形状（错开并发、崩溃可观测）。
    Ok(state.dispatcher.purge(request_id, id).await)
}

/// 归档墓碑的容量统计（R102A）。**只读**——不删任何东西。
///
/// 原矩阵设想的是"整文件 compact 物理清除墓碑"，那与"未修改行逐字节保留"和
/// "禁止全文重序列化"两条不变量同时冲突，已被否决。这里只回答"能清理多少"，
/// 具体的删除由用户在归档抽屉里**逐条**决定。
#[tauri::command]
async fn vault_capacity(state: tauri::State<'_, ShellState>) -> Result<CapacityView, String> {
    // 这个统计要读**整个** `inbox.md` 再解析，是这一组只读命令里最重的一条；
    // 同步执行会把主线程卡在读盘上。路径先取出来，闭包里只用普通 `PathBuf`。
    let path = state.vault.join("inbox.md");
    tauri::async_runtime::spawn_blocking(move || {
        slip_store::capacity::archived_capacity_at(&path)
            .map(|capacity| CapacityView {
                archived: capacity.archived,
                archived_bytes: capacity.archived_bytes,
                total_records: capacity.total_records,
                total_bytes: capacity.total_bytes,
                ratio: capacity.archived_ratio(),
            })
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| format!("容量统计被中断：{error}"))?
}

/// 容量统计的 IPC 形状。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct CapacityView {
    archived: usize,
    archived_bytes: u64,
    total_records: usize,
    total_bytes: u64,
    ratio: f64,
}

/// 读取损坏证据文件的前 64 KiB，供只读预览（R106）。
///
/// **安全约束**：只允许读 **vault 目录里**、文件名以 `inbox.md.corrupt-` 开头的文件。
/// 不加这两道检查的话，这个命令就是一个可以读任意文件的洞——参数是前端传进来的字符串。
fn guard_corrupt_path(vault: &std::path::Path, raw: &str) -> Result<std::path::PathBuf, String> {
    let candidate = std::path::Path::new(raw);
    let name = candidate
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "路径没有文件名".to_owned())?;
    if !name.starts_with("inbox.md.corrupt-") {
        return Err("只能预览 inbox.md.corrupt-* 证据文件".to_owned());
    }
    // **先做廉价词法判断**：前端传进来的可能是 `\\dead-host\share\...`，
    // 直接 canonicalize 会等 SMB 超时，整个 IPC 卡住。只要父目录词法上不是
    // vault，就立刻拒绝，连磁盘都不碰。
    let trim_sep = |text: &str| text.trim_end_matches(['/', '\\']).to_owned();
    let parent = candidate
        .parent()
        .ok_or_else(|| "路径没有父目录".to_owned())?;
    if trim_sep(&parent.to_string_lossy()) != trim_sep(&vault.to_string_lossy()) {
        return Err("只能预览 vault 目录下的文件".to_owned());
    }
    // 词法通过后再规范化**最终路径**，挡住符号链接与 `..` 的剩余情形。
    //
    // 只 canonicalize 父目录是不够的：`vault/inbox.md.corrupt-x` 可以是一个指向
    // vault 外部的符号链接，父目录检查照样通过，随后 `read_corrupt_preview`
    // 就把任意文件的内容读进 webview 了。解析到真实文件之后再看它到底在哪。
    let resolved = candidate.canonicalize().map_err(|error| {
        // 不存在的文件无法规范化；这是最常见的失败（备份已被清理、名字拼错），
        // 必须给出可分辨的说法，而不是把系统错误裸露给前端。
        if error.kind() == std::io::ErrorKind::NotFound {
            String::from("证据文件不存在")
        } else {
            format!("无法解析证据文件路径：{error}")
        }
    })?;
    let vault = vault
        .canonicalize()
        .map_err(|_| "vault 目录不可用".to_owned())?;
    if resolved.parent() != Some(vault.as_path()) {
        return Err("只能预览 vault 目录下的文件".to_owned());
    }
    // 目录也能通过父目录比对（名字恰好以 `inbox.md.corrupt-` 开头），
    // 但读目录会得到一个与内容无关的 IO 错误。先说清楚它不是文件。
    if !resolved.is_file() {
        return Err("证据路径不是普通文件".to_owned());
    }
    // 符号链接可以在 vault 内换目标：`inbox.md.corrupt-1 -> inbox.md` 的父目录
    // 检查一样通过。契约是"只预览证据文件"，所以真实文件名也必须带前缀。
    let resolved_name = resolved
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "路径没有文件名".to_owned())?;
    if !resolved_name.starts_with("inbox.md.corrupt-") {
        return Err("只能预览 inbox.md.corrupt-* 证据文件".to_owned());
    }
    Ok(resolved)
}

#[tauri::command]
async fn corrupt_preview(
    state: tauri::State<'_, ShellState>,
    path: String,
) -> Result<String, String> {
    let vault = state.vault.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let path = guard_corrupt_path(&vault, &path)?;
        slip_store::fsops::read_corrupt_preview(&path).map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| format!("读取证据被中断：{error}"))?
}

#[tauri::command]
fn vault_path(state: tauri::State<'_, ShellState>) -> String {
    state.vault.to_string_lossy().into_owned()
}

// ---------------------------------------------------------------- 启动

/// 在阻塞线程里完成启动读盘：建目录、构造 store、读偏好。
///
/// **这里的失败都不能 panic**。`ensure_dir` 失败只记错误然后继续构造 store：
/// `MarkdownStore::new` 内部走 `startup_state`，它会把"读不到"落成
/// `Degraded + 诊断`，界面横幅能显示原因；而 panic 会让整个进程在用户
/// 双击之后直接消失（release 下连控制台都没有）。
fn bootstrap_vault(vault: &std::path::Path) -> (Arc<MarkdownStore>, slip_store::prefs::Prefs) {
    if let Err(kind) = slip_store::fsops::ensure_dir(vault) {
        tracing::error!(?kind, path = %vault.display(), "cannot create vault directory; continuing degraded");
    }
    let store = Arc::new(MarkdownStore::new(vault.join("inbox.md")));
    // 偏好只在启动时读一次；运行期唯一的写入点是 `system::persist_hotkey`
    // （换备选热键成功时写回），所以"读一次"指的是这条读盘路径。
    let prefs = JsonPrefsStore::new(vault.join("ui-state.json"));
    let loaded = prefs.load_prefs();
    (store, loaded)
}

/// 初始化日志。
///
/// 在此之前项目里 16 处 `tracing::error!` 全是**静默**的：写了 tracing 却没有
/// subscriber，等于没有日志。而 `RoundtripError`、`DurabilityUnconfirmed`、
/// 托盘/热键安装失败这些恰恰是最需要留痕的路径。
///
/// 级别用 `SLIP_LOG` 覆盖（`error` / `warn` / `info` / `debug` / `trace`），默认 `info`。
/// 写到 stderr：stdout 留给 `[slip]` 那几行给人看的启动摘要。
fn init_tracing() {
    let level = match std::env::var("SLIP_LOG")
        .unwrap_or_else(|_| String::from("info"))
        .to_ascii_lowercase()
        .as_str()
    {
        "error" => tracing::Level::ERROR,
        "warn" => tracing::Level::WARN,
        "debug" => tracing::Level::DEBUG,
        "trace" => tracing::Level::TRACE,
        _ => tracing::Level::INFO,
    };
    let _ = tracing_subscriber::fmt()
        .with_max_level(level)
        .with_writer(std::io::stderr)
        .try_init();
}

pub fn run() {
    init_tracing();
    tauri::Builder::default()
        // 单实例**必须第一个注册**：它要在其它插件建窗口之前判定"是不是第二个实例"。
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            // 第二个实例被拦下时把已有窗口唤到前面，而不是再开一个。
            hotkey::summon(app);
        }))
        .plugin(
            tauri_plugin_autostart::Builder::new()
                // 带标记参数，让本进程能区分「开机被拉起」和「用户手动双击」。
                // 没有这个区分，`visible: false` 就等于双击后什么也不发生。
                .args([AUTOSTART_FLAG])
                .build(),
        )
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        // 只用于「复制诊断」：在 Rust 侧把诊断写进剪贴板。
        // webview **不授**剪贴板权限——能少给一项能力就少给一项。
        .plugin(tauri_plugin_clipboard_manager::init())
        .invoke_handler(tauri::generate_handler![
            get_snapshot,
            capture,
            complete,
            complete_many,
            edit,
            set_locked,
            set_pinned,
            archive,
            unarchive,
            undo,
            reload,
            isolate_and_reset,
            vault_path,
            vault_capacity,
            purge_archived,
            corrupt_preview,
            corrupt_files,
            reveal_corrupt,
            open_url,
            hotkey_status,
            copy_diagnostics,
            export_markdown,
            external_diff,
            backup_list,
            restore_backup,
            hotkey::dismiss_window,
        ])
        .setup(|app| {
            let vault = resolve_vault();
            let prefs_path = vault.join("ui-state.json");
            // 启动 I/O 放到阻塞线程：UI 线程只等结果，不亲自读盘。
            let init_vault = vault.clone();
            let (store, loaded) =
                match std::thread::spawn(move || bootstrap_vault(&init_vault)).join() {
                    Ok(started) => started,
                    Err(panic) => {
                        // 线程 panic 不该让进程一起死：Windows release 是
                        // `windows_subsystem = "windows"`，panic 只进 stderr——用户看到的是
                        // "双击没反应"。退回到"同路径构造 store + 默认偏好"，
                        // 让窗口照常出现，读不到的问题由 `startup_state` 落成 Degraded 诊断。
                        //
                        // `join()` 的错误是 `Box<dyn Any>`，没有 Display：手动取字符串，
                        // 取不到就写个占位，日志里不能只剩一个类型名。
                        let reason = panic
                            .downcast_ref::<String>()
                            .map(String::as_str)
                            .or_else(|| panic.downcast_ref::<&str>().copied())
                            .unwrap_or("<非字符串 panic 载荷>");
                        tracing::error!("vault initialization thread panicked: {reason}");
                        (
                            Arc::new(MarkdownStore::new(vault.join("inbox.md"))),
                            slip_store::prefs::Prefs::default(),
                        )
                    }
                };
            // 热键的合法性属于外壳：存储层只存字符串。这里做解析 + 回落。
            let hotkey = hotkey::resolve_shortcut(loaded.hotkey.as_deref());
            let initial = store.snapshot();
            let view = PaperViewSet::from_prefs(&loaded);
            let presenter = Presenter::with_view(initial, view);

            // 调度器要一个长期运行时；Tauri 自己的 async_runtime 不能保证
            // 生命周期与这里一致，所以显式建一个并持有它。
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .expect("failed to build the shell runtime");
            let dispatcher = {
                let _guard = runtime.enter();
                Dispatcher::spawn(store, presenter, 64)
            };

            println!("[slip] vault: {}", vault.display());
            println!("[slip] prefs: {}", prefs_path.display());
            // 先注册再托管：托盘只能展示真正注册成功的字符串。
            let hotkey_registered = match hotkey::register(app, &hotkey) {
                Ok(()) => true,
                Err(error) => {
                    tracing::error!("hotkey registration failed: {error}");
                    false
                }
            };
            if hotkey_registered {
                println!("[slip] hotkey: {hotkey}");
            } else {
                println!("[slip] hotkey: <未注册>");
            }
            app.manage(ShellState {
                dispatcher,
                _runtime: runtime,
                vault,
                hotkey: std::sync::Mutex::new(if hotkey_registered {
                    hotkey
                } else {
                    String::new()
                }),
            });

            // 托盘失败不应让应用起不来：最坏情况是回到"只能手动找窗口"。
            if let Err(error) = system::install_tray(app) {
                tracing::error!("tray install failed: {error}");
            }

            // `skipTaskbar` 写在 tauri.conf.json 里，但**实测没生效**：窗口创建后扩展样式是
            // `WS_EX_APPWINDOW` 而不是 `WS_EX_TOOLWINDOW`，意味着它会出现在任务栏与 Alt+Tab
            // （R71 验收时抓到的）。召唤式便签常驻任务栏会干扰用户，这里显式再设一次——
            // 配置没生效时它是兜底；将来 Tauri 修好了也只是重复设置一次，无副作用。
            match app.get_webview_window("main") {
                Some(window) => match window.set_skip_taskbar(true) {
                    Ok(()) => tracing::info!("skip_taskbar: applied"),
                    Err(error) => tracing::error!("skip_taskbar failed: {error}"),
                },
                None => tracing::error!("skip_taskbar: main window missing"),
            }

            // 托盘摘要随快照更新。
            //
            // 前端是**拉取式**的（命令返回后自己 `refresh()`），没有快照推送事件；
            // 而本项目刻意不引入回调扩展点。所以这里用一个低频轮询：每秒半看一次，
            // **只在计数变化时**才动托盘——避免无谓的 UI 调用，也避免用户正在展开的菜单被打断。
            {
                let tray_handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    let mut last: Option<usize> = None;
                    loop {
                        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
                        let pending = {
                            let state = tray_handle.state::<ShellState>();
                            match state.dispatcher.get_snapshot().await {
                                Ok(snapshot) => {
                                    snapshot.slips.iter().filter(|slip| !slip.done).count()
                                }
                                Err(_) => continue,
                            }
                        };
                        if last == Some(pending) {
                            continue;
                        }
                        last = Some(pending);
                        system::refresh_tray(&tray_handle, pending);
                    }
                });
            }

            // 手动启动（双击 exe）时把窗口显示出来：召唤式应用静默启动会让用户
            // 以为没跑起来。开机自启被拉起时才真正静默。
            //
            // 用 `show_on_launch` 而不是 `summon`：启动展示不挂"失焦自动收走"。
            // 本机实测 Clash Verge 每约 2.7 秒抢一次前台，挂上该规则会让窗口在
            // 用户看清之前就被收走，表现成"启动后一个窗口都没有"。
            // 判定规则在 slip_app::launch（可单测）：启动参数判错的两个方向代价不对称，
            // 而这只能在真机开机/双击时复现，所以必须能在单测里覆盖。
            let launched_by_autostart =
                slip_app::launch::launched_by_autostart(std::env::args_os());
            if !launched_by_autostart {
                hotkey::show_on_launch(app.handle());
            }

            // 启动自检走 tracing::debug：平时不吵，排查时 `SLIP_LOG=debug` 就有。
            // 诊断「进程在跑但看不到窗口」时，这几行能和外部枚举到的窗口做对照。
            if let Some(window) = app.get_webview_window("main") {
                diag::log_window(&window);
                diag::log_monitors(&window);
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("failed to run the Slip shell");
}

#[cfg(test)]
mod tests {
    use super::{bootstrap_vault, guard_corrupt_path};
    use slip_store::{Availability, Store};

    /// 正常证据文件：必须原样放行，且返回的是规范化后的路径。
    #[test]
    fn guard_accepts_a_regular_corrupt_file_inside_the_vault() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("inbox.md.corrupt-1");
        std::fs::write(&path, b"evidence").expect("write");

        let guarded = guard_corrupt_path(dir.path(), path.to_str().unwrap()).expect("accepted");
        assert_eq!(
            guarded,
            path.canonicalize().expect("canonical"),
            "返回的必须是规范化后的真实路径，后续读取才对得上"
        );
    }

    /// 名字前缀不对：直接拒绝，连 canonicalize 都不值得做。
    #[test]
    fn guard_rejects_files_that_are_not_evidence() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("notes.md");
        std::fs::write(&path, b"mine").expect("write");
        assert!(guard_corrupt_path(dir.path(), path.to_str().unwrap()).is_err());
    }

    /// `..` 穿越：最终路径解析后必然落在 vault 外。
    #[test]
    fn guard_rejects_dotdot_traversal() {
        let dir = tempfile::tempdir().expect("temp dir");
        let vault = dir.path().join("vault");
        std::fs::create_dir(&vault).expect("vault");
        let outside = dir.path().join("inbox.md.corrupt-1");
        std::fs::write(&outside, b"outside").expect("write");

        let raw = vault.join("..").join("inbox.md.corrupt-1");
        let error = guard_corrupt_path(&vault, raw.to_str().unwrap()).expect_err("rejected");
        assert!(error.contains("vault 目录下"), "{error}");
    }

    /// 指向 vault 外的符号链接：父目录在 vault 内也没用，必须按**真实**位置判定。
    #[cfg(unix)]
    #[test]
    fn guard_rejects_a_symlink_pointing_outside_the_vault() {
        let dir = tempfile::tempdir().expect("temp dir");
        let vault = dir.path().join("vault");
        std::fs::create_dir(&vault).expect("vault");
        let outside = dir.path().join("secret.txt");
        std::fs::write(&outside, b"outside").expect("write");
        let link = vault.join("inbox.md.corrupt-1");
        std::os::unix::fs::symlink(&outside, &link).expect("symlink");

        let error = guard_corrupt_path(&vault, link.to_str().unwrap()).expect_err("rejected");
        assert!(error.contains("vault 目录下"), "{error}");
    }

    /// vault 内的符号链接指向 vault 内的**非证据文件**：位置对了，但契约仍是"只预览证据"。
    #[cfg(unix)]
    #[test]
    fn guard_rejects_a_symlink_to_a_non_evidence_file_inside_the_vault() {
        let dir = tempfile::tempdir().expect("temp dir");
        let target = dir.path().join("inbox.md");
        std::fs::write(&target, b"live data").expect("write");
        let link = dir.path().join("inbox.md.corrupt-1");
        std::os::unix::fs::symlink(&target, &link).expect("symlink");

        assert!(guard_corrupt_path(dir.path(), link.to_str().unwrap()).is_err());
    }

    /// 文件不存在时 `canonicalize` 会失败：要给出可分辨的说法，而不是系统错误原文。
    #[test]
    fn guard_reports_a_missing_evidence_file_clearly() {
        let dir = tempfile::tempdir().expect("temp dir");
        let missing = dir.path().join("inbox.md.corrupt-404");
        let error =
            guard_corrupt_path(dir.path(), missing.to_str().unwrap()).expect_err("rejected");
        assert!(error.contains("不存在"), "{error}");
    }

    /// 目录名字碰巧符合前缀：读目录会得到无关的 IO 错误，必须先说清"不是文件"。
    #[test]
    fn guard_rejects_a_directory_named_like_evidence() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("inbox.md.corrupt-dir");
        std::fs::create_dir(&path).expect("mkdir");
        let error = guard_corrupt_path(dir.path(), path.to_str().unwrap()).expect_err("rejected");
        assert!(error.contains("不是普通文件"), "{error}");
    }

    /// `backup_bytes` 写出的备份必须与"调用方手里的字节"逐字节一致。
    ///
    /// 这条守的是"复用刚读到的内容"的正确性：调用方拿冲突检测读到的 `Vec<u8>`
    /// 去备份，备份里就应当正好是那份内容，而不是写盘瞬间重新读到的另一份。
    /// （正常路径下两者相同；差别只在测试里显式模拟：先读字节、再改文件，
    /// 传旧字节进去，备份必须是旧字节。）
    #[test]
    fn backup_bytes_writes_exactly_the_bytes_it_was_given() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("inbox.md");
        std::fs::write(&path, b"version one\n").expect("seed");

        let seen = std::fs::read(&path).expect("read");
        // 文件在读出之后又变了：备份必须记录"读到的"那一份。
        std::fs::write(&path, b"version two\n").expect("overwrite");

        let backup = slip_store::fsops::backup_bytes(&path, &seen).expect("backup written");
        assert_eq!(std::fs::read(&backup).expect("read backup"), seen);
        assert_eq!(
            std::fs::read(&path).expect("read live"),
            b"version two\n",
            "备份不得改动当前文件"
        );
    }

    /// 启动时 inbox 路径指向"普通文件下的子路径"：`ensure_dir` 与创建都会失败，
    /// 但必须返回一个 **Degraded + 非空诊断** 的 store，而不是 panic。
    #[test]
    fn startup_with_unreadable_path_degrades_instead_of_panicking() {
        let dir = tempfile::tempdir().expect("temp dir");
        let blocker = dir.path().join("blocker");
        std::fs::write(&blocker, b"not a directory").expect("write blocker");
        // `blocker/inbox` 无法存在：中间那层是文件。
        let vault = blocker.join("vault");

        let (store, prefs) = bootstrap_vault(&vault);

        let snapshot = store.snapshot();
        assert_eq!(
            snapshot.health.availability,
            Availability::Degraded,
            "读不到 vault 时必须降级而不是假装可写"
        );
        assert!(
            !snapshot.diagnostics.is_empty(),
            "降级必须带诊断，否则界面上只剩一条看不懂的横幅"
        );
        assert!(prefs.hotkey.is_none(), "读不到偏好时回默认值");
    }
}
