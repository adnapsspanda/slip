use std::path::{Path, PathBuf};
use std::time::SystemTime;

const REBUILD_HINT: &str = "\n\n\
    ================ 前端产物缺失或过期 ================\n\
    `src-tauri/tauri.conf.json` 的 frontendDist 指向 `ui/dist`，\n\
    它在编译期被嵌进二进制；产物缺失或比源码旧都会让界面悄悄停在上一版。\n\
    \n\
    先构建前端（ui/dist 不入库，每次干净克隆都要做一次）：\n\
    \n\
        cd ui\n\
        bun install --frozen-lockfile\n\
        bun run build\n\
        cd ..\n\
        cargo build -p slip-shell\n\
    \n\
    （bun 不在 PATH 时用绝对路径，见 AGENTS.md）\n\
    ======================================================\n\n";

/// 目录里最新的修改时间（递归；只用于 ui/src 这种小目录）。
fn newest_mtime(path: &Path, newest: &mut Option<(SystemTime, PathBuf)>) {
    let Ok(meta) = std::fs::metadata(path) else {
        return;
    };
    if meta.is_file() {
        if let Ok(modified) = meta.modified()
            && newest
                .as_ref()
                .is_none_or(|(current, _)| modified > *current)
        {
            *newest = Some((modified, path.to_path_buf()));
        }
        return;
    }
    let Ok(entries) = std::fs::read_dir(path) else {
        return;
    };
    for entry in entries.flatten() {
        newest_mtime(&entry.path(), newest);
    }
}

/// 前置检查：`frontendDist` 必须已经构建好，而且**不能比源码旧**。
///
/// 只查"文件存在"是不够的：改完 `ui/src` 直接 `cargo run -p slip-shell` 会把旧界面
/// 编进去，而编译、clippy、测试全都不会报错。这里比较 mtime，过期就 panic 并打印
/// 重新构建命令。`ui/dist` 在编译期由 `tauri::generate_context!` 读取。
fn main() {
    // 没有这几行，cargo 不会因为 UI 源码变化而重跑本脚本——mtime 检查永远看不到
    // "源码比产物新"。`tauri-build` 只为 dist 发 rerun-if-changed，不管 src。
    for path in [
        "../ui/src",
        "../ui/index.html",
        "../ui/package.json",
        "../ui/bun.lock",
        "../ui/vite.config.js",
        "../ui/vite.config.ts",
    ] {
        println!("cargo:rerun-if-changed={path}");
    }

    let ui = Path::new("../ui");
    let dist = ui.join("dist");
    let dist_index = dist.join("index.html");
    if !dist_index.is_file() {
        panic!("{REBUILD_HINT}");
    }

    let mut newest = None;
    newest_mtime(&ui.join("src"), &mut newest);
    for extra in [
        "index.html",
        "package.json",
        "bun.lock",
        "vite.config.js",
        "vite.config.ts",
    ] {
        let path = ui.join(extra);
        if let Ok(modified) = std::fs::metadata(&path).and_then(|meta| meta.modified())
            && newest
                .as_ref()
                .is_none_or(|(current, _)| modified > *current)
        {
            newest = Some((modified, path));
        }
    }

    let dist_time = std::fs::metadata(&dist_index)
        .and_then(|meta| meta.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH);
    if let Some((source_time, source_path)) = newest
        && source_time > dist_time
    {
        panic!(
            "{REBUILD_HINT}\n最新的前端源文件：{}\n`ui/dist/index.html` 比它旧。",
            source_path.display()
        );
    }

    tauri_build::build();
}
