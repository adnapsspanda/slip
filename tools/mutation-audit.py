#!/usr/bin/env python3
"""R79 变异审计：对三处核心不变量逐个注入变异，确认测试真的变红。

为什么用脚本而不是手工三步：手工做的时候，"恢复"这一步最容易被跳过或恢复错——
而错误的恢复比不恢复更危险（你以为代码是干净的）。

用法（在 Windows 侧）：python tools/mutation-audit.py
它会在每一步改文件、跑 WSL 测试、断言失败、再恢复，最后核对恢复后的内容与原文件**逐字节一致**。
"""
import atexit
import io
import os
import subprocess
import sys

# Windows 控制台默认按 GBK 输出，中文会乱码。统一成 UTF-8。
try:
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
except Exception:
    pass

REPO = r"D:\Dev\simple-todo"
WSL_TEST = "sh /mnt/d/Dev/simple-todo/tools/wsl-test.sh"

CASES = [
    {
        "name": "码点计数（chars().count → len）",
        "file": r"crates\slip-domain\src\text.rs",
        "old": "    text.chars().count()\n",
        "new": "    text.len() // [MUTATION]\n",
        "filter": "slip-domain",
        "expect_fail_marker": "FAILED",
    },
    {
        "name": "归档前置条件（未完成不得归档）",
        "file": r"crates\slip-domain\src\state.rs",
        "old": "            if !slip.done {\n                return Err(DomainError::ArchiveRequiresDone);\n            }\n",
        "new": "            // [MUTATION] 去掉归档前置条件\n",
        "filter": "slip-domain",
        "expect_fail_marker": "FAILED",
    },
    {
        "name": "列表前缀剥离（strip_list_prefix 变恒等）",
        "file": r"crates\slip-domain\src\split.rs",
        "old": "fn strip_list_prefix(line: &str) -> &str {\n    let s = line.trim_start();\n",
        "new": "fn strip_list_prefix(line: &str) -> &str {\n    return line; // [MUTATION]\n    #[allow(unreachable_code)]\n    let s = line.trim_start();\n",
        "filter": "slip-domain",
        "expect_fail_marker": "FAILED",
    },
    # ---- R129：扩到 10 处 ----
    {
        "name": "标签前缀判定（任意位置 # 都算标签 → URL 锚点被误判）",
        "file": r"crates\slip-domain\src\tags.rs",
        "old": "        let Some(rest) = token.strip_prefix('#') else {\n            continue;\n        };",
        "new": "        let Some(pos) = token.find('#') else { continue };\n        let rest = &token[pos + 1..]; // [MUTATION]",
        "filter": "slip-domain",
        "expect_fail_marker": "FAILED",
    },
    {
        "name": "定稿保护（锁只锁正文：去掉 SlipLocked 拦截）",
        "file": r"crates\slip-domain\src\state.rs",
        "old": "            if slip.locked {\n                return Err(DomainError::SlipLocked);\n            }\n            let text = normalize_text(raw)?;",
        "new": "            // [MUTATION] 去掉锁定保护\n            let text = normalize_text(raw)?;",
        "filter": "slip-domain",
        "expect_fail_marker": "FAILED",
    },
    {
        "name": "容量统计口径（只看 done，忽略 archived_at）",
        "file": r"crates\slip-store\src\capacity.rs",
        "old": "        if slip.done && slip.archived_at.is_some() {",
        "new": "        if slip.done { // [MUTATION]",
        "filter": "slip-store",
        "expect_fail_marker": "FAILED",
    },
    {
        "name": "置顶 marker 写入（去掉尾部兜底）",
        "file": r"crates\slip-store\src\doc.rs",
        "old": "    if slip.pinned && !wrote_pinned {\n        out.push_str(\";pinned=1\");\n    }",
        "new": "    // [MUTATION] 去掉置顶写入",
        "filter": "slip-store",
        "expect_fail_marker": "FAILED",
    },
    {
        "name": "备份保底保留（去掉\"更早那天的最后一份\"）",
        "file": r"crates\slip-store\src\fsops.rs",
        "old": "        .filter(|path| !keep_paths.iter().any(|kept| kept == *path));",
        "new": "        .filter(|_| false); // [MUTATION]",
        "filter": "slip-store",
        "expect_fail_marker": "FAILED",
    },
    {
        "name": "彻底移除的归档保护（允许删未归档条目）",
        "file": r"crates\slip-store\src\lib.rs",
        "old": "        if !(slip.done && slip.archived_at.is_some()) {",
        "new": "        if false { // [MUTATION]",
        "filter": "slip-store",
        "expect_fail_marker": "FAILED",
    },
    {
        "name": "撤销白名单（把置顶也当成可撤销）",
        "file": r"crates\slip-app\src\dispatch.rs",
        "old": "                        | Patch::Unarchive,\n                    ..",
        "new": "                        | Patch::Unarchive\n                        | Patch::SetPinned(_),\n                    ..",
        "filter": "slip-app",
        "expect_fail_marker": "FAILED",
    },
]


def run(cmd):
    # 输出是 UTF-8（cargo 的中文测试名），而 Windows 默认按 GBK 解码会直接抛异常。
    result = subprocess.run(
        cmd, shell=True, capture_output=True, text=True, encoding="utf-8", errors="replace"
    )
    return result.returncode, (result.stdout or "") + (result.stderr or "")


# 退出时的兜底还原。
#
# 为什么要它：脚本一旦异常退出（例如被早关的管道打断、或 Ctrl+C），
# 逐用例的 `finally` 可能来不及跑完，仓库里就留下**改坏的代码**——
# 这是这类工具最危险的失败形态（你以为代码是干净的）。
# 所以除了逐用例恢复，再挂一张全局底牌。
_ORIGINALS: dict[str, str] = {}


def restore_all() -> None:
    for path, original in list(_ORIGINALS.items()):
        try:
            current = io.open(path, encoding="utf-8").read()
            if current != original:
                io.open(path, "w", encoding="utf-8", newline="").write(original)
                print(f"[SAFETY] 已兜底还原 {os.path.basename(path)}")
        except OSError as error:
            print(f"[SAFETY] 还原 {path} 失败：{error}")


atexit.register(restore_all)


def main():
    failures = []
    for case in CASES:
        path = f"{REPO}\\{case['file']}"
        original = io.open(path, encoding="utf-8").read()
        _ORIGINALS[path] = original
        if case["old"] not in original:
            print(f"[SKIP] {case['name']}：锚点未匹配，跳过（并视为失败）")
            failures.append(case["name"] + "（锚点未匹配）")
            continue

        mutated = original.replace(case["old"], case["new"], 1)
        io.open(path, "w", encoding="utf-8", newline="").write(mutated)
        try:
            code, out = run(f'wsl.exe -d Arch -- bash -lc "cd /mnt/d/Dev/simple-todo && {WSL_TEST}"')
            red = (code != 0) or (case["expect_fail_marker"] in out)
            # 提取失败的测试名，作为"哪个守卫抓住了它"的证据
            caught = [line.strip() for line in out.splitlines() if "FAILED" in line][:3]
            if red:
                print(f"[PASS] {case['name']}：变异后测试变红")
                for line in caught:
                    print(f"         {line}")
            else:
                print(f"[FAIL] {case['name']}：变异后测试仍然全绿——这个不变量没有守卫")
                failures.append(case["name"])
        finally:
            io.open(path, "w", encoding="utf-8", newline="").write(original)
            restored = io.open(path, encoding="utf-8").read()
            if restored != original:
                print(f"[FAIL] {case['file']} 恢复失败：内容与原文件不一致")
                failures.append(case["name"] + "（恢复失败）")

    # 全部恢复后必须重新全绿
    code, out = run(f'wsl.exe -d Arch -- bash -lc "cd /mnt/d/Dev/simple-todo && {WSL_TEST}"')
    if code == 0:
        print(f"[PASS] {len(CASES)} 处变异全部还原后，测试恢复全绿")
    else:
        print("[FAIL] 还原后测试仍是红的——恢复过程有问题")
        failures.append("还原后未恢复全绿")

    print()
    if failures:
        print("变异审计未通过：" + "、".join(failures))
        sys.exit(1)
    print(f"变异审计通过：{len(CASES)} 处核心不变量都有活的守卫。")


if __name__ == "__main__":
    main()
