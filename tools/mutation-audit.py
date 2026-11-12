#!/usr/bin/env python3
"""R79 变异审计：对一处核心不变量注入变异，确认测试真的变红。

为什么用脚本而不是手工三步：手工做的时候，"恢复"这一步最容易被跳过或恢复错——
而错误的恢复比不恢复更危险（你以为代码是干净的）。

用法（任意平台，仓库根目录或任意位置均可）：

    python3 tools/mutation-audit.py

它会在每一步改文件、跑该 crate 的测试、断言失败、再恢复，最后核对恢复后的内容
与原文件**逐字节一致**，并复跑一次确认全绿。

判据很要紧：**只有 `test result: FAILED` 才算「测试变红」**。
变异把代码写成编译不过（`error[E...]` / `could not compile`）是「无效变异」——
它什么也没证明，必须计入失败，绝不能当成 PASS。旧实现用 `returncode != 0` 判定，
于是"写坏语法"也能通过，这是变异审计最没有价值的失败形态。
"""
import atexit
import io
import os
import subprocess
import sys
from pathlib import Path

# Windows 控制台默认按 GBK 输出，中文会乱码。统一成 UTF-8。
try:
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
except Exception:
    pass

# 仓库根目录由脚本位置推导：不再硬编码 `D:\Dev\simple-todo` 或 `/mnt/d/...`。
# 旧实现换个机器第一步就抛 FileNotFoundError，而门禁工具自己坏掉时**看起来像没事**。
REPO = Path(__file__).resolve().parents[1]

# 每个用例自带 crate 名，直接调 cargo；不再经过 `wsl.exe -d Arch` 与 `bash -lc`。

CASES = [
    {
        "name": "码点计数（chars().count → len）",
        "file": "crates/slip-domain/src/text.rs",
        "old": "    text.chars().count()\n",
        "new": "    text.len() // [MUTATION]\n",
        "crate": "slip-domain",
    },
    {
        "name": "归档前置条件（未完成不得归档）",
        "file": "crates/slip-domain/src/state.rs",
        "old": "            if !slip.done {\n                return Err(DomainError::ArchiveRequiresDone);\n            }\n",
        "new": "            // [MUTATION] 去掉归档前置条件\n",
        "crate": "slip-domain",
    },
    {
        "name": "列表前缀剥离（strip_list_prefix 变恒等）",
        "file": "crates/slip-domain/src/split.rs",
        "old": "fn strip_list_prefix(line: &str) -> &str {\n    let s = line.trim_start();\n",
        "new": "fn strip_list_prefix(line: &str) -> &str {\n    return line; // [MUTATION]\n    #[allow(unreachable_code)]\n    let s = line.trim_start();\n",
        "crate": "slip-domain",
    },
    # ---- R129：扩到 10 处 ----
    {
        "name": "标签前缀判定（任意位置 # 都算标签 → URL 锚点被误判）",
        "file": "crates/slip-domain/src/tags.rs",
        "old": "        let Some(rest) = token.strip_prefix('#') else {\n            continue;\n        };",
        "new": "        let Some(pos) = token.find('#') else { continue };\n        let rest = &token[pos + 1..]; // [MUTATION]",
        "crate": "slip-domain",
    },
    {
        "name": "定稿保护（锁只锁正文：去掉 SlipLocked 拦截）",
        "file": "crates/slip-domain/src/state.rs",
        "old": "            if slip.locked {\n                return Err(DomainError::SlipLocked);\n            }\n            let text = normalize_text(raw)?;",
        "new": "            // [MUTATION] 去掉锁定保护\n            let text = normalize_text(raw)?;",
        "crate": "slip-domain",
    },
    {
        "name": "容量统计口径（只看 done，忽略 archived_at）",
        "file": "crates/slip-store/src/capacity.rs",
        "old": "        if slip.done && slip.archived_at.is_some() {",
        "new": "        if slip.done { // [MUTATION]",
        "crate": "slip-store",
    },
    {
        "name": "置顶 marker 写入（去掉尾部兜底）",
        "file": "crates/slip-store/src/doc.rs",
        "old": "    if slip.pinned && !wrote_pinned {\n        out.push_str(\";pinned=1\");\n    }",
        "new": "    // [MUTATION] 去掉置顶写入",
        "crate": "slip-store",
    },
    {
        "name": "备份保底保留（去掉\"更早那天的最后一份\"）",
        "file": "crates/slip-store/src/fsops.rs",
        "old": "        .filter(|path| !keep_paths.iter().any(|kept| kept == *path));",
        "new": "        .filter(|_| false); // [MUTATION]",
        "crate": "slip-store",
    },
    {
        "name": "彻底移除的归档保护（允许删未归档条目）",
        "file": "crates/slip-store/src/lib.rs",
        "old": "        if !(slip.done && slip.archived_at.is_some()) {",
        "new": "        if false { // [MUTATION]",
        "crate": "slip-store",
    },
    {
        "name": "撤销白名单（把置顶也当成可撤销）",
        "file": "crates/slip-app/src/dispatch.rs",
        "old": "                        | Patch::Unarchive,\n                    ..",
        "new": "                        | Patch::Unarchive\n                        | Patch::SetPinned(_),\n                    ..",
        "crate": "slip-app",
    },
]


def cargo_test(crate: str):
    """跑一个 crate 的全部测试。返回 (退出码, 合并输出)。"""
    cmd = [
        "cargo",
        "test",
        "-p",
        crate,
        "--all-targets",
        "--offline",
    ]
    # 输出是 UTF-8（cargo 的中文测试名），而 Windows 默认按 GBK 解码会直接抛异常。
    result = subprocess.run(
        cmd,
        cwd=REPO,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    return result.returncode, (result.stdout or "") + (result.stderr or "")


def classify(code: int, out: str):
    """把一次变异后的测试运行分成三种结局。

    - ``red``：测试真的变红，说明这条不变量有活的守卫；
    - ``invalid``：变异让代码编译不过（或根本没跑起来），什么也没证明；
    - ``green``：测试全绿，说明守卫缺失。
    """
    if "test result: FAILED" in out:
        return "red"
    if "error[E" in out or "could not compile" in out or "could not compile" in out.lower():
        return "invalid"
    if code != 0:
        # 非零退出但不是测试失败：多半是编译/环境问题，同样不算证据。
        return "invalid"
    return "green"


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

    # 先确认基线是绿的：基线不绿时任何"变异后变红"都不成立。
    print("== 基线：先确认测试是绿的 ==")
    involved = sorted({case["crate"] for case in CASES})
    for crate in involved:
        code, out = cargo_test(crate)
        if classify(code, out) == "green":
            print(f"  [PASS] {crate} 基线全绿")
        else:
            print(f"  [FAIL] {crate} 基线不是绿的，无法进行变异审计")
            sys.exit(1)
    print()

    for case in CASES:
        path = REPO / case["file"]
        original = io.open(path, encoding="utf-8").read()
        _ORIGINALS[str(path)] = original
        if case["old"] not in original:
            print(f"[SKIP] {case['name']}：锚点未匹配，跳过（并视为失败）")
            failures.append(case["name"] + "（锚点未匹配）")
            continue

        mutated = original.replace(case["old"], case["new"], 1)
        io.open(path, "w", encoding="utf-8", newline="").write(mutated)
        try:
            code, out = cargo_test(case["crate"])
            verdict = classify(code, out)
            # 提取失败的测试名，作为"哪个守卫抓住了它"的证据
            caught = [line.strip() for line in out.splitlines() if "FAILED" in line][:3]
            if verdict == "red":
                print(f"[PASS] {case['name']}：变异后测试变红")
                for line in caught:
                    print(f"         {line}")
            elif verdict == "invalid":
                print(
                    f"[FAIL] {case['name']}：变异让代码编译不过——这什么也没证明，"
                    "请把变异写成语义变更而不是语法错误"
                )
                for line in out.splitlines():
                    if "error[E" in line or "could not compile" in line:
                        print(f"         {line.strip()}")
                        break
                failures.append(case["name"] + "（无效变异）")
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
    print()
    print("== 收尾：全部还原后再跑一次 ==")
    okay = True
    for crate in involved:
        code, out = cargo_test(crate)
        if classify(code, out) != "green":
            okay = False
            print(f"[FAIL] {crate} 还原后不是绿的——恢复过程有问题")
    if okay:
        print(f"[PASS] {len(CASES)} 处变异全部还原后，测试恢复全绿")
    else:
        failures.append("还原后未恢复全绿")

    print()
    if failures:
        print("变异审计未通过：" + "、".join(failures))
        sys.exit(1)
    print(f"变异审计通过：{len(CASES)} 处核心不变量都有活的守卫。")


if __name__ == "__main__":
    main()
