//! 平台差异的唯一出口。
//!
//! 外壳里所有"只有某个系统才有的东西"都收在这里：家目录变量名、默认打开方式、
//! 在文件管理器里定位文件、本地时区偏移。其余模块只调用这里的语义化函数，
//! 不出现 `explorer.exe` / `xdg-open` 之类的字面量——否则每加一个平台就要在
//! 业务入口里再写一遍 `#[cfg]`。
//!
//! 硬约束：
//! - **不阻塞 UI 线程**：打开动作一律只 `spawn`、不 `wait`。
//! - **不引入新依赖**：Windows 侧沿用系统自带命令，Unix 侧用 `xdg-open`。

use std::io;
use std::path::{Path, PathBuf};

use time::UtcOffset;

/// 用户家目录。取不到环境变量时返回 `None`，由调用方决定退化值。
///
/// Windows 用 `USERPROFILE`（`HOME` 在原生 Windows 进程里通常不存在）；
/// 其余平台用 `HOME`。
pub fn home_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    const HOME_VAR: &str = "USERPROFILE";
    #[cfg(not(windows))]
    const HOME_VAR: &str = "HOME";

    std::env::var_os(HOME_VAR).map(PathBuf::from)
}

/// 由家目录推出 vault 路径。
///
/// 与原有语义一致：`Documents` 存在就用 `<home>/Documents/Slip`，
/// 否则用 `<home>/Slip`。`documents_is_dir` 由调用方探测后传入，
/// 这样这个函数保持纯逻辑，能被单测覆盖两条分支。
pub fn vault_from(home: &Path, documents_is_dir: bool) -> PathBuf {
    let base = if documents_is_dir {
        home.join("Documents")
    } else {
        home.to_path_buf()
    };
    base.join("Slip")
}

/// 用系统默认程序打开一个路径（文件或目录）。
///
/// 只启动子进程，不等待：菜单回调跑在 UI 线程上，等子进程退出会卡住整个托盘。
pub fn open_path(path: &Path) -> io::Result<()> {
    #[cfg(windows)]
    let mut command = std::process::Command::new("explorer.exe");
    #[cfg(not(windows))]
    let mut command = std::process::Command::new("xdg-open");

    command.arg(path).spawn().map(|_| ())
}

/// 用系统默认浏览器打开一个 URL。
///
/// 只启动子进程，不等待。校验（仅 http/https、无空白/控制字符）由调用方负责；
/// 这里只负责"怎么开"。
pub fn open_url(url: &str) -> io::Result<()> {
    #[cfg(windows)]
    let mut command = {
        // URL 作为**单个 argv** 传递，不经过 `cmd /c start`：
        // 后者会把 `&` 当命令分隔符，那是注入。
        let mut command = std::process::Command::new("rundll32.exe");
        command.arg("url.dll,FileProtocolHandler");
        command
    };
    #[cfg(not(windows))]
    let mut command = std::process::Command::new("xdg-open");

    command.arg(url).spawn().map(|_| ())
}

/// 在文件管理器里定位一个文件。
///
/// Windows 传 `explorer.exe /select,<path>`，且 `/select,<path>` 必须是**一个**
/// 参数，否则 explorer 会把路径当成要独立打开的目标。非 Windows 没有等价的
/// "选中"语义，退化为打开它所在的父目录。
pub fn reveal(path: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        let arg = format!("/select,{}", path.display());
        std::process::Command::new("explorer.exe")
            .arg(arg)
            .spawn()
            .map(|_| ())
    }
    #[cfg(not(windows))]
    {
        match path.parent() {
            Some(parent) => open_path(parent),
            // 没有父目录时至少把路径本身交给默认程序，总好过什么都不做。
            None => open_path(path),
        }
    }
}

/// 从 `reg query` 的输出里取出 `ActiveTimeBias` 的值（分钟，`UTC = 本地 + bias`）。
///
/// 纯函数，因为这段解析最该被测：注册表给的是十六进制 DWORD，负偏移是补码
/// （中国时区是 `0xfffffe20`，即 −480）。算错符号会让文件名差 16 小时。
#[cfg(windows)]
fn parse_active_bias(raw: &str) -> Option<i32> {
    let token = raw
        .split_whitespace()
        .find(|token| token.starts_with("0x") || token.starts_with("0X"))?;
    let hex = token
        .trim_start_matches("0x")
        .trim_start_matches("0X")
        .trim_end_matches(|c: char| !c.is_ascii_hexdigit());
    let value = u32::from_str_radix(hex, 16).ok()?;
    Some(value as i32)
}

/// Windows：读注册表里的 `ActiveTimeBias`（只读一次，进程内缓存）。
#[cfg(windows)]
fn local_offset_minutes() -> Option<i32> {
    use std::sync::OnceLock;

    static CACHE: OnceLock<Option<i32>> = OnceLock::new();
    *CACHE.get_or_init(|| {
        let output = std::process::Command::new("reg.exe")
            .args([
                "query",
                r"HKLM\SYSTEM\CurrentControlSet\Control\TimeZoneInformation",
                "/v",
                "ActiveTimeBias",
            ])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        parse_active_bias(&String::from_utf8_lossy(&output.stdout))
    })
}

/// 本机当前相对 UTC 的偏移；拿不到时返回 `None`，由调用方决定回落值。
///
/// Windows 走注册表而不是 `time` 的本地时区查询：`ActiveTimeBias` 是系统按
/// **当前生效**的时区与夏令时写下的有效偏移，正好是导出文件名要的东西。
/// 其余平台直接问 `time`（工作区的 `time` 已开启 `local-offset`）。
pub fn local_offset() -> Option<UtcOffset> {
    #[cfg(windows)]
    {
        let bias = local_offset_minutes()?;
        UtcOffset::from_whole_seconds(-bias * 60).ok()
    }
    #[cfg(not(windows))]
    {
        time::OffsetDateTime::now_local()
            .ok()
            .map(|local| local.offset())
    }
}

#[cfg(test)]
mod tests {
    use super::vault_from;
    use std::path::Path;

    /// Documents 存在时，vault 落在它下面——这是绝大多数用户的分支。
    #[test]
    fn vault_prefers_documents_when_it_exists() {
        let home = Path::new("/home/alice");
        assert_eq!(
            vault_from(home, true),
            Path::new("/home/alice/Documents/Slip")
        );
    }

    /// 家目录里没有 Documents 时退回 `<home>/Slip`，不能凭空造一层。
    #[test]
    fn vault_falls_back_to_home_without_documents() {
        let home = Path::new("/home/alice");
        assert_eq!(vault_from(home, false), Path::new("/home/alice/Slip"));
    }

    #[cfg(windows)]
    mod windows_bias {
        use super::super::parse_active_bias;

        /// 负偏移（中国是 UTC+8）走补码：0xfffffe20 = −480 分钟。
        #[test]
        fn active_bias_parses_twos_complement_hex() {
            let raw = "\r\nHKEY_LOCAL_MACHINE\\SYSTEM\\CurrentControlSet\\Control\\TimeZoneInformation\r\n    ActiveTimeBias    REG_DWORD    0xfffffe20\r\n";
            assert_eq!(parse_active_bias(raw), Some(-480));
            assert_eq!(
                parse_active_bias("    ActiveTimeBias    REG_DWORD    0x1e0"),
                Some(480)
            );
        }

        /// 读不到或格式变了就返回 None，交给调用方退回 UTC——绝不能猜。
        #[test]
        fn active_bias_returns_none_for_unexpected_output() {
            assert_eq!(parse_active_bias(""), None);
            assert_eq!(parse_active_bias("错误: 系统找不到指定的注册表项"), None);
            assert_eq!(parse_active_bias("ActiveTimeBias REG_DWORD 0xzz"), None);
        }
    }
}
