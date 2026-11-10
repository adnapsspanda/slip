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
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

use crate::{Hash, IoKind};

/// 生成不覆盖名字时的最大尝试次数。达到上限说明目录里已堆满同名证据文件。
const MAX_NAME_ATTEMPTS: u32 = 1000;

pub(crate) fn map_io(error: std::io::Error) -> IoKind {
    // Windows 的共享冲突与锁冲突落在 Rust 的 `Uncategorized` 里，光看 `kind()`
    // 只能得到 `Other`——那个词对排查毫无帮助。按原生码区分出来，才能重试、才能解释。
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
/// 只识别 `*.bak-<unix_ms>[-n]` 形式的文件；按 (时间戳, 序号) 排序，
/// 不按文件名做字典序比较（`-10` 与 `-2` 的字典序是错的）。
pub fn prune_backups(dir: &Path, keep: usize) -> Result<(), IoKind> {
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
    let stale = backups.len() - keep;
    for (_, _, path) in backups.into_iter().take(stale) {
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(map_io(error)),
        }
    }
    Ok(())
}

/// 从 `inbox.md.bak-<unix_ms>[-n]` 解析出 `(unix_ms, n)`。
fn parse_backup_name(name: &str) -> Option<(u64, u32)> {
    let index = name.rfind(".bak-")?;
    let mut parts = name[index + ".bak-".len()..].split('-');
    let stamp = parts.next()?.parse().ok()?;
    let n = match parts.next() {
        Some(raw) => raw.parse().ok()?,
        None => 1,
    };
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

        prune_backups(dir.path(), 5).unwrap();

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

    #[test]
    fn prune_backups_is_a_noop_when_under_limit() {
        let dir = temp_dir();
        write_file(&dir.path().join("inbox.md.bak-1"), b"x");
        assert_eq!(prune_backups(dir.path(), 5), Ok(()));
        assert!(dir.path().join("inbox.md.bak-1").exists());
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
}
