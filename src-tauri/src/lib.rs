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
    pub(crate) hotkey: String,
}

/// 开机自启时传给本进程的标记参数。
///
/// 窗口配置是 `visible: false`（召唤式），但那样用户双击 exe 会「什么也没发生」。
/// 用这个参数区分：开机拉起 → 静默驻留托盘；手动双击 → 直接把窗口显示出来。
const AUTOSTART_FLAG: &str = "--autostart";

/// 解析 vault 目录。
///
/// 用可见路径而不是隐藏的 appdata：Markdown 是真源，用户随时会想用编辑器打开它。
/// `SLIP_VAULT` 可覆盖，用于测试与多套并行实例。
fn resolve_vault() -> PathBuf {
    if let Some(custom) = std::env::var_os("SLIP_VAULT") {
        return PathBuf::from(custom);
    }
    let home = std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let documents = home.join("Documents");
    let base = if documents.is_dir() { documents } else { home };
    base.join("Slip")
}

fn rejected(request_id: &str, code: &str, message: impl Into<String>) -> Ack {
    Ack {
        request_id: request_id.to_owned(),
        result: AckResult::Rejected,
        ui_rev: None,
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
fn corrupt_files(state: tauri::State<'_, ShellState>) -> Vec<String> {
    system::corrupt_artifacts(&state.vault)
        .into_iter()
        .filter_map(|path| path.file_name().map(|n| n.to_string_lossy().into_owned()))
        .collect()
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
    let path = system::write_export(&state.vault, &content)?;
    // 导出完直接定位到文件：用户下一步多半就是复制或打开它。
    let arg = format!("/select,{}", path.display());
    let _ = std::process::Command::new("explorer.exe").arg(arg).spawn();
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
    let disk: Vec<(String, String, bool)> = slip_store::peek_slips(&path)
        .map_err(|kind| format!("读取 inbox 失败：{kind}"))?
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
#[tauri::command]
fn backup_list(state: tauri::State<'_, ShellState>) -> Vec<BackupRow> {
    slip_store::fsops::list_backups(&state.vault)
        .into_iter()
        .map(|info| BackupRow {
            name: info.name,
            size: info.size,
            stamp_ms: info.stamp_ms,
            slips: info.slips,
            preview: info.preview,
        })
        .collect()
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
    let hotkey = if state.hotkey.is_empty() {
        "<未注册>"
    } else {
        state.hotkey.as_str()
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

#[tauri::command]
fn vault_path(state: tauri::State<'_, ShellState>) -> String {
    state.vault.to_string_lossy().into_owned()
}

// ---------------------------------------------------------------- 启动

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
            edit,
            set_locked,
            archive,
            unarchive,
            undo,
            reload,
            isolate_and_reset,
            vault_path,
            corrupt_files,
            reveal_corrupt,
            open_url,
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
            let (store, loaded) = std::thread::spawn(move || {
                if let Err(kind) = slip_store::fsops::ensure_dir(&init_vault) {
                    panic!("cannot create vault {init_vault:?}: {kind:?}");
                }
                let store = Arc::new(MarkdownStore::new(init_vault.join("inbox.md")));
                // 偏好只在启动时读一次：热键由外壳解析，运行期不再写回。
                let prefs = JsonPrefsStore::new(init_vault.join("ui-state.json"));
                let loaded = prefs.load_prefs();
                (store, loaded)
            })
            .join()
            .expect("vault initialization failed");
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
                hotkey: if hotkey_registered {
                    hotkey
                } else {
                    String::new()
                },
            });

            // 托盘失败不应让应用起不来：最坏情况是回到"只能手动找窗口"。
            if let Err(error) = system::install_tray(app) {
                tracing::error!("tray install failed: {error}");
            }

            // 手动启动（双击 exe）时把窗口显示出来：召唤式应用静默启动会让用户
            // 以为没跑起来。开机自启被拉起时才真正静默。
            //
            // 用 `show_on_launch` 而不是 `summon`：启动展示不挂"失焦自动收走"。
            // 本机实测 Clash Verge 每约 2.7 秒抢一次前台，挂上该规则会让窗口在
            // 用户看清之前就被收走，表现成"启动后一个窗口都没有"。
            let launched_by_autostart = std::env::args().any(|arg| arg == AUTOSTART_FLAG);
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
