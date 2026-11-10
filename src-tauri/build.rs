use std::path::Path;

/// 前置检查：`frontendDist` 必须已经构建好。
///
/// `tauri::generate_context!` 会在编译期读 `../ui/dist`，目录不存在时它只抛
/// 一句 `proc macro panicked` —— 对刚克隆仓库的人完全没有指向性。
/// 这里提前一步失败，把「该跑什么命令」直接说出来。
fn main() {
    let dist = Path::new("../ui/dist");
    if !dist.join("index.html").is_file() {
        panic!(
            "\n\n\
             ================ 前端产物缺失 ================\n\
             `src-tauri/tauri.conf.json` 的 frontendDist 指向 `ui/dist`，\n\
             但 `ui/dist/index.html` 不存在，所以外壳无法编译。\n\
             \n\
             先构建前端（ui/dist 不入库，每次干净克隆都要做一次）：\n\
             \n\
                 cd ui\n\
                 bun install\n\
                 bun run build\n\
                 cd ..\n\
                 cargo build -p slip-shell\n\
             \n\
             （bun 不在 PATH 时用绝对路径，见 AGENTS.md）\n\
             ==============================================\n\n"
        );
    }

    tauri_build::build();
}
