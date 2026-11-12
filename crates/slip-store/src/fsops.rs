//! 面向磁盘的最小原语。所有文件系统调用集中在这里。
//!
//! 三条边界（v11 §3.4、§4.4、§4.5）：
//!
//! 1. **提交点 = 原子替换成功**。替换成功之后的同步失败只表示"持久性未确认"，
//!    绝不表示"未提交"；因此 [`write_atomic`] 用返回值区分这两件事，而不是只用 `Err`。
//! 2. **备份与隔离绝不覆盖**已有文件。重名一律递增 `-2`/`-3`，哪怕同一毫秒内多次操作。
//! 3. **Windows 不伪造父目录同步**。"平台不提供这种确认"与"确认失败"是两件事，
//!    后者才置 `DurabilityUnconfirmed`。

use std::ffi::{OsStr, OsString};
use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};

use crate::{Hash, IoKind};

/// 生成不覆盖名字时的最大尝试次数。达到上限说明目录里已堆满同名证据文件。
const MAX_NAME_ATTEMPTS: u32 = 1000;

pub(crate) fn map_io(error: std::io::Error) -> IoKind {
    // Windows 的共享冲突与锁冲突落在 Rust 的 `Uncategorized` 里，光看 `kind()`
    // 只能得到 `Other`——那个词对排查毫无帮助。按原生码区分出来，才能重试、才能解释。
    //
    // **只在 Windows 上**按原生码判断：Linux 上同一个数字是别的错误（32 = EPIPE、
    // 33 = EDOM），无条件比较会把它们误判成"文件被占用"，进而白等重试。
    #[cfg(windows)]
    if let Some(code) = error.raw_os_error() {
        // ERROR_SHARING_VIOLATION = 32, ERROR_LOCK_VIOLATION = 33
        if code == 32 || code == 33 {
            return IoKind::Busy;
        }
    }
    match error.kind() {
        ErrorKind::NotFound => IoKind::NotFound,
        ErrorKind::AlreadyExists => IoKind::Occupied,
        ErrorKind::PermissionDenied => IoKind::PermissionDenied,
        _ => IoKind::Other,
    }
}

/// 取出父目录。裸相对文件名（如 `inbox.md`）的父目录是空路径，按当前目录处理。
pub(crate) fn parent_dir(path: &Path) -> &Path {
    match path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir,
        _ => Path::new("."),
    }
}

/// 确保 vault 目录存在。目录创建也属于磁盘写，外壳不得绕过本 crate（I1）。
pub fn ensure_dir(path: &Path) -> Result<(), IoKind> {
    fs::create_dir_all(path).map_err(map_io)
}

/// 当前 Unix 毫秒。测试可用 [`failpoints::pin_now_millis`] 固定，便于复现重名冲突。
fn now_millis() -> u64 {
    #[cfg(test)]
    if let Some(pinned) = failpoints::take_pinned_now_millis() {
        return pinned;
    }
    let nanos = time::OffsetDateTime::now_utc().unix_timestamp_nanos();
    u64::try_from(nanos / 1_000_000).unwrap_or(0)
}

/// `base<suffix>`，`n == 1` 时不带递增后缀。
fn sibling_name(base: &OsStr, tag: &str, stamp: u64, n: u32) -> OsString {
    let mut name = OsString::from(base);
    name.push(format!(".{tag}-{stamp}"));
    if n > 1 {
        name.push(format!("-{n}"));
    }
    name
}

/// 读取整个文件的原始字节。
pub fn read_bytes(path: &Path) -> Result<Vec<u8>, IoKind> {
    fs::read(path).map_err(map_io)
}

/// 计算磁盘原始字节的 BLAKE3 摘要。
///
/// 对**磁盘上的全部原始字节**求值：BOM、CRLF、非记录行、非法 UTF-8 普通行都参与，
/// 不对解析后的值求值时（v11 I16）。
pub fn hash_bytes(bytes: &[u8]) -> Hash {
    Hash(*blake3::hash(bytes).as_bytes())
}

/// 原子替换：同目录建 tmp → 写入 → `sync_all` → `persist`/rename。
///
/// **`Err` 只表示未提交**；替换成功之后的持久化确认由调用方用 [`sync_dir`] 单独完成
/// （§4.4：提交点之后可处理的失败不得报成 `Err`）。
///
/// `tempfile::NamedTempFile::persist` **不会** fsync 文件或目录，所以这里先自己
/// `sync_all()`，再替换。
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), IoKind> {
    let dir = parent_dir(path);
    let mut tmp = tempfile::NamedTempFile::new_in(dir).map_err(map_io)?;
    tmp.write_all(bytes).map_err(map_io)?;
    tmp.as_file().sync_all().map_err(map_io)?;

    // 故障注入点（R111）：临时文件**已经写满并 sync 过**，但还没替换过去。
    // 这是"写到一半断电"能构造出的最接近的形态——此时原文件必须一个字节都没变。
    #[cfg(test)]
    if failpoints::take_write_interruption() {
        return Err(IoKind::Other);
    }

    // 提交点：这一行返回成功之后，文件已经是新内容。
    tmp.persist(path).map_err(|err| map_io(err.error))?;
    Ok(())
}

/// 同目录 tmp + 原子替换，**不 fsync**（偏好专用，v11 §3.5）。
pub fn replace_without_sync(path: &Path, bytes: &[u8]) -> Result<(), IoKind> {
    let mut tmp = tempfile::NamedTempFile::new_in(parent_dir(path)).map_err(map_io)?;
    tmp.write_all(bytes).map_err(map_io)?;
    tmp.persist(path).map_err(|err| map_io(err.error))?;
    Ok(())
}

/// 同步父目录项。
///
/// Unix 上尽力尝试；Windows v1 **不提供**父目录同步确认（§4.4），直接返回 `Ok(())`，
/// 不伪造一次失败事件。
#[cfg(unix)]
fn sync_dir_impl(dir: &Path) -> Result<(), IoKind> {
    std::fs::File::open(dir)
        .and_then(|f| f.sync_all())
        .map_err(map_io)
}

#[cfg(not(unix))]
fn sync_dir_impl(_dir: &Path) -> Result<(), IoKind> {
    Ok(())
}

/// 同步父目录；见 [`sync_dir_impl`] 的平台差异说明。
pub fn sync_dir(dir: &Path) -> Result<(), IoKind> {
    #[cfg(test)]
    if failpoints::take_dir_sync_failure() {
        return Err(IoKind::Other);
    }
    sync_dir_impl(dir)
}

/// 生成不覆盖的备份文件 `inbox.md.bak-<unix_ms>`，返回实际路径。
///
/// 重名递增 `-2`/`-3`，以 `create_new` 语义创建，**绝不覆盖**已有备份。
pub fn backup(path: &Path) -> Result<PathBuf, IoKind> {
    #[cfg(test)]
    if failpoints::take_backup_failure() {
        return Err(IoKind::Other);
    }

    let bytes = read_bytes(path)?;
    let dir = parent_dir(path);
    let base = path.file_name().ok_or(IoKind::Other)?;
    let stamp = now_millis();

    for n in 1..=MAX_NAME_ATTEMPTS {
        let candidate = dir.join(sibling_name(base, "bak", stamp, n));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(mut file) => {
                if let Err(error) = file.write_all(&bytes) {
                    drop(file);
                    // 半成品不算证据，清掉；清不掉也不影响主流程。
                    let _ = fs::remove_file(&candidate);
                    return Err(map_io(error));
                }
                return Ok(candidate);
            }
            Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(map_io(error)),
        }
    }
    Err(IoKind::Occupied)
}

/// 把现有文件移动到不覆盖的 `corrupt-<ts>` 路径，返回新路径。
///
/// **只对读得到的无效文件调用**，不得对已成功解析的文件调用（v11 §3.4）。
/// 对**有效**文件做 rename 会把用户的正常内容变成"隔离证据"，
/// 这是 v11 明确禁止的（I20）。
///
/// 实现先以 `create_new` 占住目标名字，再 rename 覆盖**自己刚创建的空占位**，
/// 因此不会覆盖任何已存在的证据文件，也不依赖平台的 rename 语义是否原子替换。
pub fn isolate(path: &Path) -> Result<PathBuf, IoKind> {
    move_aside(path, "corrupt")
}

/// 通用"移到一边"：`<name>.<tag>-<unix_ms>`，重名递增，绝不覆盖。
///
/// 供内容隔离（`corrupt`）与偏好损坏留档（`bad`）共用。
pub fn move_aside(path: &Path, tag: &str) -> Result<PathBuf, IoKind> {
    let dir = parent_dir(path);
    let base = path.file_name().ok_or(IoKind::Other)?;
    let stamp = now_millis();

    for n in 1..=MAX_NAME_ATTEMPTS {
        let candidate = dir.join(sibling_name(base, tag, stamp, n));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(placeholder) => {
                // Windows 上不能 rename 到仍打开的文件，先关掉占位句柄。
                drop(placeholder);
                return match fs::rename(path, &candidate) {
                    Ok(()) => Ok(candidate),
                    Err(error) => {
                        let _ = fs::remove_file(&candidate);
                        Err(map_io(error))
                    }
                };
            }
            Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(map_io(error)),
        }
    }
    Err(IoKind::Occupied)
}

/// 以 `create_new(true)` 创建空文件；已存在则返回 `IoKind::Occupied`。
///
/// 提交点是创建成功本身（v11 §3.4）。创建后的目录同步由调用方决定，
/// 它不是提交的一部分。
pub fn create_empty_new(path: &Path) -> Result<(), IoKind> {
    match OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(file) => {
            drop(file);
            Ok(())
        }
        Err(error) => Err(map_io(error)),
    }
}

/// 保留最近 `keep` 代备份，删除更旧的。失败不影响主流程。
///
/// 一个备份文件的可展示信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupInfo {
    /// 裸文件名，例如 `inbox.md.bak-1700000000000`。
    pub name: String,
    pub size: u64,
    /// 文件名里的毫秒时间戳。
    pub stamp_ms: u64,
    /// 同一毫秒内的序号（`-2` / `-3`），没有则为 1。
    pub seq: u32,
    /// 里面有多少条记录。**这是区分两代备份最要紧的信息**：
    /// 只显示体积时，5 个"1.2 KB"的备份长得一模一样，很容易恢复错。
    pub slips: usize,
    /// 第一条记录的正文（截断），给人一眼认出来的钩子。
    pub preview: String,
}

/// 摘要用的单行预览：压掉换行，最长 24 个字符。
fn preview_of(text: &str) -> String {
    let flat: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= 24 {
        return flat;
    }
    let head: String = flat.chars().take(24).collect();
    format!("{head}…")
}

/// 列出目录里的备份，**新的在前**。
pub fn list_backups(dir: &Path) -> Vec<BackupInfo> {
    let mut found = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return found;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some((stamp_ms, seq)) = parse_backup_name(&name) else {
            continue;
        };
        let size = entry.metadata().map(|meta| meta.len()).unwrap_or(0);
        // 读不出内容不算致命：备份仍可恢复，只是没有摘要可显示。
        let (slips, preview) = match read_bytes(&entry.path()) {
            Ok(bytes) => {
                let parsed = crate::doc::parse(&bytes);
                let count = parsed.document.records.len();
                let first = parsed
                    .document
                    .records
                    .first()
                    .map(|record| preview_of(&record.slip.text))
                    .unwrap_or_default();
                (count, first)
            }
            Err(_) => (0, String::new()),
        };
        found.push(BackupInfo {
            name,
            size,
            stamp_ms,
            seq,
            slips,
            preview,
        });
    }
    // 新的大在前；同毫秒则序号大的在前。
    found.sort_by_key(|info| std::cmp::Reverse((info.stamp_ms, info.seq)));
    found
}

/// 用指定备份覆盖 `inbox`。
///
/// **只接受本目录下真实存在的备份名**：名字里的 `/`、`\`、`..` 一律拒绝，
/// 且必须能被 [`parse_backup_name`] 认出来——否则这就是一个"用任意路径覆盖 inbox"
/// 的接口。
pub fn restore_backup(inbox: &Path, name: &str) -> Result<(), IoKind> {
    if name.contains('/') || name.contains('\\') || name.contains("..") {
        return Err(IoKind::PermissionDenied);
    }
    if parse_backup_name(name).is_none() {
        return Err(IoKind::NotFound);
    }
    let source = parent_dir(inbox).join(name);
    let bytes = read_bytes(&source)?;
    write_atomic(inbox, &bytes)
}

/// 只识别 `*.bak-<unix_ms>[-n]` 形式的文件；按 (时间戳, 序号) 排序，
/// 不按文件名做字典序比较（`-10` 与 `-2` 的字典序是错的）。
pub fn prune_backups(dir: &Path, keep: usize, now_ms: u64) -> Result<(), IoKind> {
    let entries = fs::read_dir(dir).map_err(map_io)?;
    let mut backups: Vec<(u64, u32, PathBuf)> = Vec::new();
    for entry in entries {
        let entry = entry.map_err(map_io)?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if let Some((stamp, n)) = parse_backup_name(name) {
            backups.push((stamp, n, entry.path()));
        }
    }

    if backups.len() <= keep {
        return Ok(());
    }

    backups.sort_by_key(|(stamp, n, _)| (*stamp, *n));

    // 保留策略（R105）：最近 `keep` 代 **加上** 至少一代"更早那天的最后一份"。
    //
    // 为什么需要后半句：只按代数保留时，一天内连点五次就会把前一天的备份全部挤掉——
    // 而"昨天还是好的、今天才发现坏了"恰恰是最常见的损坏发现方式。
    //
    // 分成"天"用的是 **UTC 天**（`stamp / 86_400_000`）：备份名里是 unix 毫秒，
    // 而时区会变（夏令时、出差改时区），按 UTC 分桶得到的边界是稳定的。
    // 代价是最坏情况下多留一代，符合"宁可多留、不要误删"的取向。
    const DAY_MS: u64 = 86_400_000;
    let today = now_ms / DAY_MS;

    let mut keep_paths: Vec<PathBuf> = Vec::new();
    // 1) 最近 keep 代
    for (_, _, path) in backups.iter().rev().take(keep) {
        keep_paths.push(path.clone());
    }
    // 2) 更早那天的最后一份（若还没在保留集里）
    let older_day_newest = backups
        .iter()
        .rev()
        .find(|(stamp, _, _)| *stamp / DAY_MS < today)
        .map(|(_, _, path)| path)
        .filter(|path| !keep_paths.iter().any(|kept| kept == *path));
    if let Some(path) = older_day_newest {
        keep_paths.push(path.clone());
    }

    let mut removed = 0usize;
    for (_, _, path) in &backups {
        if keep_paths.iter().any(|kept| kept == path) {
            continue;
        }
        match fs::remove_file(path) {
            Ok(()) => removed += 1,
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(map_io(error)),
        }
    }
    let _ = removed;
    Ok(())
}

/// 从 `inbox.md.bak-<unix_ms>[-n]` 解析出 `(unix_ms, n)`。
///
/// **必须是我们自己生成的那种名字**才认：结构不合规的一律返回 `None`。
/// 之前的实现有两处漏洞，都会让"随便一个文件"被当成备份：
/// - 多余的段被忽略（`inbox.md.bak-1-2-3` 会被读成 `(1, 2)`）；
/// - 不校验前缀与分隔符，`/`、`\` 也能混进来（回到 `restore_backup` 就是路径穿越的
///   潜在入口，虽然那里还有一道目录检查）。
fn parse_backup_name(name: &str) -> Option<(u64, u32)> {
    if name.contains('/') || name.contains('\\') {
        return None;
    }
    let index = name.rfind(".bak-")?;
    // 前缀不能为空：`.bak-1` 这种不是备份，是别的什么东西。
    if index == 0 {
        return None;
    }
    let mut parts = name[index + ".bak-".len()..].split('-');
    let stamp: u64 = parts.next()?.parse().ok()?;
    let n: u32 = match parts.next() {
        Some(raw) => raw.parse().ok()?,
        None => 1,
    };
    // 还剩下段说明这个名字不是我们生成的（我们最多两段）。
    if parts.next().is_some() {
        return None;
    }
    Some((stamp, n))
}

/// 测试用的注入点。
///
/// 只在 `cfg(test)` 下存在：用来在 `doc.rs` 尚未实现时驱动磁盘事务与恢复矩阵。
/// 生产构建里没有任何测试旁路。
#[cfg(test)]
pub(crate) mod failpoints {
    use std::cell::{Cell, RefCell};

    thread_local! {
        static BACKUP_FAILURE: Cell<bool> = const { Cell::new(false) };
        static DIR_SYNC_FAILURE: Cell<bool> = const { Cell::new(false) };
        static PINNED_NOW: RefCell<Option<u64>> = const { RefCell::new(None) };
        static WRITE_INTERRUPTION: Cell<bool> = const { Cell::new(false) };
    }

    /// 让下一次 [`super::write_atomic`] 在"临时文件已写好、尚未替换"时失败。
    pub(crate) fn fail_next_write_interruption() {
        WRITE_INTERRUPTION.with(|flag| flag.set(true));
    }

    pub(crate) fn take_write_interruption() -> bool {
        WRITE_INTERRUPTION.with(|flag| flag.replace(false))
    }

    pub(crate) fn fail_next_backup() {
        BACKUP_FAILURE.with(|flag| flag.set(true));
    }

    pub(crate) fn take_backup_failure() -> bool {
        BACKUP_FAILURE.with(|flag| flag.replace(false))
    }

    pub(crate) fn fail_next_dir_sync() {
        DIR_SYNC_FAILURE.with(|flag| flag.set(true));
    }

    pub(crate) fn take_dir_sync_failure() -> bool {
        DIR_SYNC_FAILURE.with(|flag| flag.replace(false))
    }

    /// 把 [`super::now_millis`] 固定到某个毫秒，仅对下一次调用生效。
    pub(crate) fn pin_now_millis(millis: u64) {
        PINNED_NOW.with(|slot| *slot.borrow_mut() = Some(millis));
    }

    pub(crate) fn take_pinned_now_millis() -> Option<u64> {
        PINNED_NOW.with(|slot| slot.borrow_mut().take())
    }
}

// ---------- 损坏文件预览 ----------

/// 损坏文件预览的最大长度：64 KiB。
///
/// 取值理由：一条纸条正文上限 2 KB，64 KiB 够看到几十行上下文；同时能挡住
/// "20 MB 的损坏文件把界面卡住"这种极端情况。截断按**字节**做，再由
/// `from_utf8_lossy` 把断在中间的多字节序列变成替换字符——预览本来就不是解析，
/// 允许这种损失。
pub const CORRUPT_PREVIEW_LIMIT: usize = 64 * 1024;

/// **只读**读取损坏文件的前 `CORRUPT_PREVIEW_LIMIT` 字节，转成可显示文本。
///
/// 允许 lossy：目的是让用户在弹窗里看见还剩下什么，而不是把它解析成文档。
/// 非法 UTF-8 会被替换成 U+FFFD 而不是报错——报错会让用户看不到任何内容，
/// 那恰好是这一轮要解决的问题。
///
/// **绝不**用来写回任何文件：`slip-store` 只通过常规写路径（临时文件 + rename）
/// 改 `inbox.md`，这个函数连写句柄都不打开。
pub fn read_corrupt_preview(path: &Path) -> Result<String, IoKind> {
    let file = fs::File::open(path).map_err(map_io)?;
    let mut buffer = Vec::with_capacity(8 * 1024);
    // `take` 保证不会把一个超大文件整个读进内存。
    file.take(CORRUPT_PREVIEW_LIMIT as u64)
        .read_to_end(&mut buffer)
        .map_err(map_io)?;
    Ok(String::from_utf8_lossy(&buffer).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir() -> tempfile::TempDir {
        tempfile::tempdir().expect("temp dir")
    }

    fn write_file(path: &Path, bytes: &[u8]) {
        std::fs::write(path, bytes).expect("write fixture");
    }

    // ---------- 读写与 hash ----------

    #[test]
    fn read_bytes_returns_every_byte() {
        let dir = temp_dir();
        let path = dir.path().join("inbox.md");
        let raw = b"\xEF\xBB\xBF- [ ] a\r\nplain\n\xFF\xFE";
        write_file(&path, raw);
        assert_eq!(read_bytes(&path).unwrap(), raw.to_vec());
    }

    #[test]
    fn read_bytes_reports_not_found() {
        let dir = temp_dir();
        assert_eq!(
            read_bytes(&dir.path().join("missing.md")),
            Err(IoKind::NotFound)
        );
    }

    #[test]
    fn hash_covers_bom_crlf_and_non_record_bytes() {
        // I16：BOM、CRLF、非记录行都参与 hash，不得先 trim/解码再求值。
        assert_ne!(hash_bytes(b"\xEF\xBB\xBFa"), hash_bytes(b"a"));
        assert_ne!(hash_bytes(b"a\r\nb"), hash_bytes(b"a\nb"));
        assert_ne!(hash_bytes(b"note\n"), hash_bytes(b"note"));
        assert_eq!(hash_bytes(b"abc"), hash_bytes(b"abc"));
    }

    // ---------- 原子替换 ----------

    #[test]
    fn write_atomic_replaces_content_and_leaves_no_temp_files() {
        let dir = temp_dir();
        let path = dir.path().join("inbox.md");
        write_file(&path, b"old\n");

        assert_eq!(write_atomic(&path, b"new\n"), Ok(()));
        assert_eq!(read_bytes(&path).unwrap(), b"new\n".to_vec());

        let leftovers: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .filter(|name| name != "inbox.md")
            .collect();
        assert!(leftovers.is_empty(), "残留临时文件: {leftovers:?}");
    }

    #[test]
    fn sync_dir_failure_is_observable_after_commit() {
        // §4.4：目录同步失败发生在提交点之后，调用方据此只加 `DurabilityUnconfirmed`。
        let dir = temp_dir();
        let path = dir.path().join("inbox.md");
        write_file(&path, b"old\n");

        assert_eq!(write_atomic(&path, b"new\n"), Ok(()));
        assert_eq!(read_bytes(&path).unwrap(), b"new\n".to_vec());

        failpoints::fail_next_dir_sync();
        assert_eq!(sync_dir(dir.path()), Err(IoKind::Other));
        assert_eq!(sync_dir(dir.path()), Ok(()));
    }

    // ---------- 备份 ----------

    #[test]
    fn backup_never_overwrites_same_millisecond() {
        let dir = temp_dir();
        let path = dir.path().join("inbox.md");
        write_file(&path, b"v1\n");

        failpoints::pin_now_millis(1_700_000_000_000);
        let first = backup(&path).unwrap();
        failpoints::pin_now_millis(1_700_000_000_000);
        let second = backup(&path).unwrap();

        assert_eq!(first.file_name().unwrap(), "inbox.md.bak-1700000000000");
        assert_eq!(second.file_name().unwrap(), "inbox.md.bak-1700000000000-2");
        assert_eq!(read_bytes(&first).unwrap(), b"v1\n".to_vec());
        assert_eq!(read_bytes(&second).unwrap(), b"v1\n".to_vec());
    }

    #[test]
    fn backup_captures_content_at_call_time() {
        let dir = temp_dir();
        let path = dir.path().join("inbox.md");
        write_file(&path, b"before\n");
        let first = backup(&path).unwrap();
        write_file(&path, b"after\n");
        let second = backup(&path).unwrap();

        assert_eq!(read_bytes(&first).unwrap(), b"before\n".to_vec());
        assert_eq!(read_bytes(&second).unwrap(), b"after\n".to_vec());
    }

    #[test]
    fn backup_reports_not_found_for_missing_source() {
        let dir = temp_dir();
        assert_eq!(backup(&dir.path().join("inbox.md")), Err(IoKind::NotFound));
    }

    #[test]
    fn prune_backups_keeps_newest_generations() {
        let dir = temp_dir();
        for stamp in 1..=7u64 {
            write_file(&dir.path().join(format!("inbox.md.bak-{stamp}")), b"x");
        }
        // 同一毫秒的第二代必须排在 `-1` 之后，且不能被字典序误判。
        write_file(&dir.path().join("inbox.md.bak-7-2"), b"x");
        write_file(&dir.path().join("unrelated.txt"), b"x");

        // now_ms 取一个远大于这些 stamp 的值：它们都属于更早那天。
        prune_backups(dir.path(), 5, 10_000_000_000).unwrap();

        let mut remaining: Vec<String> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        remaining.sort();
        assert_eq!(
            remaining,
            vec![
                "inbox.md.bak-4",
                "inbox.md.bak-5",
                "inbox.md.bak-6",
                "inbox.md.bak-7",
                "inbox.md.bak-7-2",
                "unrelated.txt",
            ]
        );
    }

    /// R105：一天内连点五次，不能把昨天那一代挤掉。
    #[test]
    fn prune_backups_keeps_one_from_an_earlier_day() {
        const DAY: u64 = 86_400_000;
        let now = 100 * DAY;
        let dir = temp_dir();

        write_file(
            &dir.path().join(format!("inbox.md.bak-{}", 99 * DAY + 5)),
            b"y",
        );
        for i in 0..5u64 {
            write_file(&dir.path().join(format!("inbox.md.bak-{}", now + i)), b"x");
        }

        prune_backups(dir.path(), 5, now).unwrap();

        assert!(
            dir.path()
                .join(format!("inbox.md.bak-{}", 99 * DAY + 5))
                .exists(),
            "昨天那一代被挤掉了——这正是 R105 要修的问题"
        );
    }

    /// 跨天时，被淘汰的只能是更早那天里除最后一份之外的。
    #[test]
    fn prune_backups_keeps_only_the_last_one_of_older_days() {
        const DAY: u64 = 86_400_000;
        let now = 100 * DAY;
        let dir = temp_dir();

        for (day, n) in [(98u64, 2u64), (99, 2), (100, 2)] {
            for i in 0..n {
                write_file(
                    &dir.path().join(format!("inbox.md.bak-{}", day * DAY + i)),
                    b"x",
                );
            }
        }

        prune_backups(dir.path(), 2, now).unwrap();

        let mut remaining: Vec<String> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        remaining.sort();
        assert_eq!(
            remaining,
            vec![
                format!("inbox.md.bak-{}", 99 * DAY + 1),
                format!("inbox.md.bak-{}", 100 * DAY),
                format!("inbox.md.bak-{}", 100 * DAY + 1),
            ]
        );
    }

    /// 同一天内不该因为保底逻辑而多留。
    #[test]
    fn prune_backups_does_not_duplicate_the_fallback() {
        const DAY: u64 = 86_400_000;
        let now = 100 * DAY;
        let dir = temp_dir();
        for i in 0..3u64 {
            write_file(&dir.path().join(format!("inbox.md.bak-{}", now + i)), b"x");
        }
        prune_backups(dir.path(), 2, now).unwrap();
        let count = fs::read_dir(dir.path()).unwrap().count();
        assert_eq!(count, 2, "同一天里保留 keep 代即可，不该多留");
    }

    #[test]
    fn prune_backups_is_a_noop_when_under_limit() {
        let dir = temp_dir();
        write_file(&dir.path().join("inbox.md.bak-1"), b"x");
        assert_eq!(prune_backups(dir.path(), 5, 10_000_000_000), Ok(()));
        assert!(dir.path().join("inbox.md.bak-1").exists());
    }

    // ---------- 损坏文件预览（R106） ----------

    #[test]
    fn corrupt_preview_reads_plain_text() {
        let dir = temp_dir();
        let path = dir.path().join("inbox.md.corrupt-1");
        write_file(&path, "还能看见的文字\n第二行\n".as_bytes());
        let text = read_corrupt_preview(&path).unwrap();
        assert_eq!(text, "还能看见的文字\n第二行\n");
    }

    #[test]
    fn corrupt_preview_replaces_invalid_bytes_instead_of_failing() {
        // 这条是这一轮的核心：非法 UTF-8 必须让用户**看见内容**，而不是报错。
        let dir = temp_dir();
        let path = dir.path().join("inbox.md.corrupt-2");
        write_file(&path, &[0xE4, 0xBD, 0xA0, 0xFF, 0xFE, 0x0A]); // "你" + 两个非法字节 + 换行
        let text = read_corrupt_preview(&path).unwrap();
        assert!(text.starts_with('你'), "合法前缀该原样保留：{text:?}");
        assert!(
            text.contains('\u{FFFD}'),
            "非法字节该变成替换字符：{text:?}"
        );
    }

    #[test]
    fn corrupt_preview_truncates_long_files() {
        let dir = temp_dir();
        let path = dir.path().join("inbox.md.corrupt-3");
        let big = vec![b'a'; CORRUPT_PREVIEW_LIMIT * 2 + 17];
        write_file(&path, &big);
        let text = read_corrupt_preview(&path).unwrap();
        assert_eq!(
            text.len(),
            CORRUPT_PREVIEW_LIMIT,
            "预览必须按上限截断，否则大文件会把界面卡住"
        );
    }

    #[test]
    fn corrupt_preview_reports_not_found() {
        let dir = temp_dir();
        assert_eq!(
            read_corrupt_preview(&dir.path().join("inbox.md.corrupt-nope")),
            Err(IoKind::NotFound)
        );
    }

    // ---------- 隔离 ----------

    #[test]
    fn isolate_never_overwrites_same_millisecond() {
        let dir = temp_dir();
        let path = dir.path().join("inbox.md");

        write_file(&path, b"first corrupt");
        failpoints::pin_now_millis(1_700_000_000_000);
        let first = isolate(&path).unwrap();
        assert!(!path.exists(), "隔离之后原路径必须空出来");

        write_file(&path, b"second corrupt");
        failpoints::pin_now_millis(1_700_000_000_000);
        let second = isolate(&path).unwrap();

        assert_eq!(first.file_name().unwrap(), "inbox.md.corrupt-1700000000000");
        assert_eq!(
            second.file_name().unwrap(),
            "inbox.md.corrupt-1700000000000-2"
        );
        assert_eq!(read_bytes(&first).unwrap(), b"first corrupt".to_vec());
        assert_eq!(read_bytes(&second).unwrap(), b"second corrupt".to_vec());
    }

    #[test]
    fn isolate_steps_over_preexisting_evidence() {
        let dir = temp_dir();
        let path = dir.path().join("inbox.md");
        failpoints::pin_now_millis(42);
        write_file(&dir.path().join("inbox.md.corrupt-42"), b"earlier evidence");
        write_file(&path, b"corrupt");

        let moved = isolate(&path).unwrap();
        assert_eq!(moved.file_name().unwrap(), "inbox.md.corrupt-42-2");
        assert_eq!(
            read_bytes(&dir.path().join("inbox.md.corrupt-42")).unwrap(),
            b"earlier evidence".to_vec(),
            "已有证据不得被覆盖"
        );
    }

    #[test]
    fn move_aside_handles_other_tags() {
        let dir = temp_dir();
        let path = dir.path().join("ui-state.json");
        write_file(&path, b"{broken");
        failpoints::pin_now_millis(7);

        let moved = move_aside(&path, "bad").unwrap();
        assert_eq!(moved.file_name().unwrap(), "ui-state.json.bad-7");
        assert!(!path.exists());
    }

    // ---------- 创建空文件 ----------

    #[test]
    fn create_empty_new_creates_then_reports_occupied() {
        let dir = temp_dir();
        let path = dir.path().join("inbox.md");

        assert_eq!(create_empty_new(&path), Ok(()));
        assert_eq!(read_bytes(&path).unwrap(), Vec::<u8>::new());
        assert_eq!(create_empty_new(&path), Err(IoKind::Occupied));
    }

    #[test]
    fn create_empty_new_does_not_truncate_existing_file() {
        let dir = temp_dir();
        let path = dir.path().join("inbox.md");
        write_file(&path, b"keep me");

        assert_eq!(create_empty_new(&path), Err(IoKind::Occupied));
        assert_eq!(read_bytes(&path).unwrap(), b"keep me".to_vec());
    }

    #[test]
    fn parent_dir_falls_back_to_current_directory() {
        assert_eq!(parent_dir(Path::new("inbox.md")), Path::new("."));
        assert_eq!(parent_dir(Path::new("a/inbox.md")), Path::new("a"));
    }

    #[test]
    fn list_backups_is_newest_first_and_ignores_other_files() {
        let dir = tempfile::tempdir().expect("temp dir");
        for name in [
            "inbox.md.bak-10",
            "inbox.md.bak-30",
            "inbox.md.bak-20",
            "inbox.md.bak-30-2",
            "inbox.md",
            "other.txt",
        ] {
            write_file(&dir.path().join(name), b"x");
        }
        let names: Vec<String> = list_backups(dir.path())
            .into_iter()
            .map(|b| b.name)
            .collect();
        assert_eq!(
            names,
            vec![
                "inbox.md.bak-30-2",
                "inbox.md.bak-30",
                "inbox.md.bak-20",
                "inbox.md.bak-10"
            ]
        );
    }

    #[test]
    fn list_backups_summarizes_content_so_generations_differ() {
        let dir = tempfile::tempdir().expect("temp dir");
        let first_only = concat!(
            "- [ ] 第一代只有一条 <!-- slip:v1;id=01a1c7e0-0000-7000-8000-000000000001;",
            "created=2026-11-10T00:00:00Z -->\n"
        );
        let two = format!(
            "{first_only}- [ ] 第二代多了一条 <!-- slip:v1;id=01a1c7e0-0000-7000-8000-000000000002;created=2026-11-10T01:00:00Z -->\n"
        );
        write_file(&dir.path().join("inbox.md.bak-1"), first_only.as_bytes());
        write_file(&dir.path().join("inbox.md.bak-2"), two.as_bytes());

        let list = list_backups(dir.path());
        assert_eq!(list.len(), 2);
        // 新的在前
        assert_eq!(list[0].name, "inbox.md.bak-2");
        assert_eq!(list[0].slips, 2, "第二代有两条");
        assert_eq!(list[0].preview, "第一代只有一条");
        assert_eq!(list[1].slips, 1);
    }

    /// 结构不合规的名字一个都不能被认成备份。
    #[test]
    fn parse_backup_name_rejects_malformed_names() {
        // 正常形态
        assert_eq!(
            parse_backup_name("inbox.md.bak-1700000000000"),
            Some((1700000000000, 1))
        );
        assert_eq!(
            parse_backup_name("inbox.md.bak-1700000000000-3"),
            Some((1700000000000, 3))
        );
        // 各种畸形
        for bad in [
            "inbox.md.bak-",
            "inbox.md.bak-abc",
            "inbox.md.bak--1",
            "inbox.md.bak-1700000000000-",
            "inbox.md.bak-1700000000000-1-2",
            ".bak-1700000000000",
            "inbox.md.bak-99999999999999999999999999",
        ] {
            assert_eq!(parse_backup_name(bad), None, "不该被认成备份：{bad}");
        }
        // 路径分隔符一律拒绝
        assert_eq!(parse_backup_name("a/b.bak-1"), None);
        assert_eq!(parse_backup_name("a\\b.bak-1"), None);
    }

    /// 多余段的文件既不列出，也不能被恢复——它根本不是我们生成的备份。
    #[test]
    fn malformed_backup_names_are_ignored_and_not_restorable() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("inbox.md.bak-1-2-3"), b"- [ ] x\n").unwrap();
        std::fs::write(dir.path().join("inbox.md.bak-1000-1"), b"- [ ] y\n").unwrap();

        let list = list_backups(dir.path());
        assert_eq!(list.len(), 1, "只有合规的那个应当被列出");
        assert_eq!(list[0].name, "inbox.md.bak-1000-1");

        let inbox = dir.path().join("inbox.md");
        std::fs::write(&inbox, b"").unwrap();
        assert!(
            restore_backup(&inbox, "inbox.md.bak-1-2-3").is_err(),
            "结构不合规的名字不能被恢复"
        );
    }

    #[test]
    fn restore_backup_refuses_names_outside_the_directory() {
        let dir = tempfile::tempdir().expect("temp dir");
        let inbox = dir.path().join("inbox.md");
        write_file(&inbox, b"current");
        // 这几个都不是"本目录下的备份名"，任何情况下都不该覆盖 inbox。
        for name in [
            "..\\evil.md",
            "/etc/passwd",
            "sub/inbox.md.bak-1",
            "not-a-backup.md",
        ] {
            assert!(
                restore_backup(&inbox, name).is_err(),
                "should refuse {name}"
            );
        }
        assert_eq!(read_bytes(&inbox).unwrap(), b"current");
    }

    #[test]
    fn restore_backup_overwrites_with_the_chosen_generation() {
        let dir = tempfile::tempdir().expect("temp dir");
        let inbox = dir.path().join("inbox.md");
        write_file(&inbox, b"current");
        write_file(&dir.path().join("inbox.md.bak-5"), b"older");

        restore_backup(&inbox, "inbox.md.bak-5").expect("restore");
        assert_eq!(read_bytes(&inbox).unwrap(), b"older");
    }
}
