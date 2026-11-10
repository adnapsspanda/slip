import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

// 没有 dev server 时 Tauri 直接读 dist；有 dev server 时这里只是本地预览。
// 刻意**不配** `devUrl`/`beforeDevCommand`：本项目用 `cargo run` 直跑，
// 不依赖 tauri CLI。流程是 `bun run build` → `cargo run -p slip-shell`。
export default defineConfig({
  plugins: [svelte()],
  clearScreen: false,
  build: {
    // 相对路径：Tauri 用 tauri.localhost 加载，绝对路径会 404
    assetsDir: "assets",
    emptyOutDir: true,
    target: "chrome105",
    sourcemap: false,
  },
  server: {
    port: 1420,
    strictPort: true,
  },
});
