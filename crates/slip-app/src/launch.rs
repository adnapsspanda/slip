//! 启动参数判定。
//!
//! 只有一件事：**这次是不是开机自启拉起来的**。它决定启动后要不要把窗口显示出来。
//!
//! 判错的代价不对称：
//! - 自启被当成手动启动 → 每次开机都弹一个窗口，用户会去关掉自启；
//! - 手动启动被当成自启 → 双击后什么都没有，用户以为程序坏了。
//!
//! 而这两种错都**只在真机上开机/双击时才能复现**，所以判定必须能在单测里覆盖。

use std::ffi::OsStr;

/// 自启标记参数。
pub const AUTOSTART_FLAG: &str = "--autostart";

/// 这一组启动参数里是否带自启标记。
///
/// **精确匹配整个参数**，不做前缀/子串/大小写模糊匹配：
/// - `--autostart-extra`、`--autostart=1` 都不算（将来要加变体时显式加分支，
///   而不是让判定悄悄变宽）；
/// - 路径里恰好含 `--autostart` 的（例如 `C:\tools\--autostart\x.exe`）不算；
/// - 大小写不同不算：参数是我们自己写进注册表的，就是这一种写法。
///
/// 参数类型是 `AsRef<OsStr>` 而不是 `AsRef<str>`：进程参数本来就可能是非 Unicode
/// （`std::env::args()` 遇到这种参数会 panic）。启动参数判定没有理由要求 UTF-8，
/// 用 `OsStr` 精确比较就不会让一个畸形参数把进程打死。
pub fn launched_by_autostart<I, S>(args: I) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    args.into_iter()
        .any(|arg| arg.as_ref() == OsStr::new(AUTOSTART_FLAG))
}

#[cfg(test)]
mod tests {
    use super::{AUTOSTART_FLAG, launched_by_autostart};

    #[test]
    fn plain_launch_is_not_autostart() {
        assert!(!launched_by_autostart(["slip-shell.exe"]));
        assert!(!launched_by_autostart(["slip-shell.exe", "--debug"]));
        assert!(!launched_by_autostart(Vec::<String>::new()));
    }

    #[test]
    fn exact_flag_is_autostart() {
        assert!(launched_by_autostart(["slip-shell.exe", AUTOSTART_FLAG]));
        assert!(launched_by_autostart([AUTOSTART_FLAG]));
        // 出现两次也仍然是自启。
        assert!(launched_by_autostart([
            "slip-shell.exe",
            AUTOSTART_FLAG,
            AUTOSTART_FLAG
        ]));
    }

    #[test]
    fn similar_looking_flags_are_not_autostart() {
        // 判定变宽会让"手动启动"被误当自启 → 双击后什么都不出现。
        assert!(!launched_by_autostart(["--autostart-extra"]));
        assert!(!launched_by_autostart(["--autostart=1"]));
        assert!(!launched_by_autostart(["--auto"]));
        assert!(!launched_by_autostart(["--AUTOSTART"]));
        assert!(!launched_by_autostart(["--autostart "]));
    }

    #[test]
    fn paths_containing_the_flag_do_not_count() {
        // 参数是逐个比的，不是整行子串搜索——否则路径里带这几个字符就能骗过判定。
        assert!(!launched_by_autostart([
            r"C:\tools\--autostart\x.exe",
            r"--autostart-folder",
        ]));
    }

    #[cfg(unix)]
    #[test]
    fn non_unicode_arguments_on_unix_do_not_panic() {
        // `std::env::args()` 遇到非 UTF-8 参数会 panic；判定本身不该有这种前提。
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let bogus = OsString::from_vec(vec![0xff, 0xfe]);
        assert!(!launched_by_autostart([bogus]));
    }

    #[cfg(windows)]
    #[test]
    fn non_unicode_arguments_on_windows_do_not_panic() {
        // Windows 侧的等价物：孤立代理项不是合法 UTF-16，`args()` 同样会 panic。
        use std::ffi::OsString;
        use std::os::windows::ffi::OsStringExt;

        let bogus = OsString::from_wide(&[0xD800]);
        assert!(!launched_by_autostart([bogus]));
    }
}
