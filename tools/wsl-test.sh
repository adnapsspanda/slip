#!/usr/bin/env sh
# 在 WSL 里跑「纯逻辑」crate 的测试：slip-domain / slip-store / slip-app。
#
# 为什么单独有这个脚本：
#   - 这三个 crate 明确不含 tauri，可以在 Linux 上编译运行，**完全绕开 Windows 的
#     Smart App Control**（那是本机反复踩到的坑：新编出的产物会被按文件拒绝执行）；
#   - `src-tauri`（GUI 外壳）与端到端验收仍然只能在 Windows 上跑，见 AGENTS.md。
#
# 用法（Windows 侧）：
#   wsl.exe -d Arch -- bash -lc 'sh /mnt/d/Dev/simple-todo/tools/wsl-test.sh'
#   wsl.exe -d Arch -- bash -lc 'sh /mnt/d/Dev/simple-todo/tools/wsl-test.sh -- --nocapture'
#
# 前置条件（一次性）：
#   1. WSL 里有 rustup 工具链（本机已装，版本与 rust-toolchain.toml 一致：1.99.1）；
#   2. 能拿到 crate 依赖。二选一：
#      a) WSL 内网络可达 crates.io（去掉下面的 --offline）；或
#      b) 把 Windows 的 cargo 缓存复制进来，然后离线跑（推荐，避开代理）：
#           robocopy "$env:USERPROFILE\.cargo\registry" \\wsl.localhost\Arch\home\<user>\.cargo\registry /E
#
# 目标目录默认放在 WSL 的 ext4 上：源码在 /mnt/d（9p，I/O 慢），产物放 Linux 侧能快几倍。
set -e

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$HOME/.cache/slip-target}"

# 不支持 `--` 之后的参数透传写法差异，这里做个简单归一：
#   tools/wsl-test.sh             → 默认离线
#   tools/wsl-test.sh --online    → 联网拉依赖
online=0
for arg in "$@"; do
  if [ "$arg" = "--online" ]; then
    online=1
  fi
done

if [ "$online" -eq 1 ]; then
  cargo test -p slip-domain -p slip-store -p slip-app
else
  cargo test --offline -p slip-domain -p slip-store -p slip-app
fi
