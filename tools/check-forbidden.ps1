# 检查"不可回退的约束"里那些**可以被机器查出来**的条目。
#
# 为什么需要它：`AGENTS.md` 的「不可回退的约束」里有一半以上是纯靠人记的
# （禁止引入 watcher / SQLite / 物理归档、presenter 不上锁、前端不注入 HTML）。
# 记性靠不住，而这几条一旦被破坏，代价是架构级的返工。
#
# 这里只查能被文本证据判定的部分，查不出的一律不假装覆盖
# （例如"禁止全文重序列化"要靠 format_closure 的往返测试，不在这里）。
#
# 用法：
#   pwsh -NoProfile -File tools/check-forbidden.ps1
#   pwsh -NoProfile -File tools/check-forbidden.ps1 -SelfTest   # 验证模式本身没写死
param(
    # 自检：拿正负样本跑一遍模式，防止正则写错导致守卫**永远是绿的**
    # （这种坏法最难发现：脚本每天都在跑，却什么也拦不住）。
    [switch]$SelfTest
)

$ErrorActionPreference = 'Stop'
Set-Location (Split-Path -Parent $PSScriptRoot)

$failures = @()

# 代码里那些"解释为什么不用某物"的注释会误报，先屏蔽注释再匹配。
function Mask-Comments {
    param([string]$Text, [ref]$State)
    $out = [System.Text.StringBuilder]::new($Text.Length)
    $len = $Text.Length
    $i = 0
    while ($i -lt $len) {
        if ($State.Value -eq 'html') {
            $close = $Text.IndexOf('-->', $i)
            if ($close -lt 0) { return (' ' * $len) }
            $out.Append(' ' * ($close + 3 - $i)) | Out-Null
            $i = $close + 3
            $State.Value = 'code'
            continue
        }
        if ($State.Value -eq 'block') {
            $close = $Text.IndexOf('*/', $i)
            if ($close -lt 0) { return (' ' * $len) }
            $out.Append(' ' * ($close + 2 - $i)) | Out-Null
            $i = $close + 2
            $State.Value = 'code'
            continue
        }
        # 逐字符比较而不是 Substring：旧实现每个字符都分配一次剩余串，
        # 是 O(n²) 且制造大量垃圾——整脚本 1.8 秒里大半耗在这。
        $c = $Text[$i]
        if ($c -eq '<' -and ($i + 3) -lt $len -and $Text[$i + 1] -eq '!' -and $Text[$i + 2] -eq '-' -and $Text[$i + 3] -eq '-') {
            $State.Value = 'html'; $out.Append('    ') | Out-Null; $i += 4; continue
        }
        if ($c -eq '/' -and ($i + 1) -lt $len -and $Text[$i + 1] -eq '*') {
            $State.Value = 'block'; $out.Append('  ') | Out-Null; $i += 2; continue
        }
        if ($c -eq '/' -and ($i + 1) -lt $len -and $Text[$i + 1] -eq '/') {
            $out.Append(' ' * ($len - $i)) | Out-Null
            break
        }
        $out.Append($c) | Out-Null
        $i++
    }
    return $out.ToString()
}

# ---------------------------------------------------------------------------
# 文件索引：**只枚举一次**，且在枚举阶段就剪掉大目录。
#
# 为什么改这一处：旧实现是"每个检查各自 Get-ChildItem -Recurse 扫全仓库"，
# 而 target/ 里有几万个构建产物——过滤写在 Where-Object 里，枚举时照样全走一遍。
# 实测整脚本要 4.2 秒，而门禁是每次提交前都要跑的。
# ---------------------------------------------------------------------------
$script:fileIndex = $null
function Get-SourceIndex {
    if ($null -ne $script:fileIndex) { return $script:fileIndex }
    $files = [System.Collections.Generic.List[string]]::new()
    $skipDirs = @('\target\', '\node_modules\', '\.git\', '\dist\')
    $stack = [System.Collections.Generic.Stack[string]]::new()
    # **必须用 ProviderPath**：在 `\\wsl.localhost\...` 这类 UNC 路径下，
    # `(Get-Location).Path` 会带上 `Microsoft.PowerShell.Core\FileSystem::` 前缀，
    # `Directory.EnumerateFiles` 直接抛错，索引变成空表——所有检查恒绿。
    # 本机就是这么踩到的：加了 notify 也不报。ProviderPath 才是纯文件系统路径。
    $stack.Push((Get-Location).ProviderPath)
    while ($stack.Count -gt 0) {
        $dir = $stack.Pop()
        try {
            foreach ($file in [System.IO.Directory]::EnumerateFiles($dir)) { $files.Add($file) }
            foreach ($child in [System.IO.Directory]::EnumerateDirectories($dir)) {
                $hit = $false
                foreach ($skip in $skipDirs) { if ($child.Contains($skip)) { $hit = $true; break } }
                if (-not $hit) { $stack.Push($child) }
            }
        }
        catch {
            continue
        }
    }
    $script:fileIndex = $files
    return $files
}

# 文件内容缓存：7 个检查会反复读同一批文件，读一遍就够。
# 旧实现每次检查都 ReadAllLines 一遍，实测占了大半时间。
$script:lineCache = @{}
function Get-SourceLines([string]$Path) {
    if ($script:lineCache.ContainsKey($Path)) { return $script:lineCache[$Path] }
    $lines = [System.IO.File]::ReadAllLines($Path)
    $script:lineCache[$Path] = $lines
    return $lines
}

# 把 `-Include` 的两种写法分开：
#   · `*.rs` / `*.svelte` → 按**扩展名**匹配（`.rs`）；
#   · `Cargo.toml` / `tauri.conf.json` → 按**完整文件名**精确匹配。
#
# 旧实现把两种都塞进「扩展名集合」，于是 `Cargo.toml` 被当成扩展名 `.toml` 永远不会
# 与 `GetExtension` 的结果相等——那三条守卫（watcher / SQLite / CSP）曾经**恒为通过**。
# 含 `?`、`[` 等复杂 glob 一律拒绝：静默退回 `.Like` 只会让下一次改动又悄悄失效。
function Split-IncludePatterns {
    param([string[]]$Include)
    $exts = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::OrdinalIgnoreCase)
    $names = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::OrdinalIgnoreCase)
    foreach ($glob in $Include) {
        if ($glob -match '[?\[\]]') {
            throw "Include 只支持 '*.ext' 与精确文件名两种写法，收到：$glob"
        }
        if ($glob.StartsWith('*.')) {
            [void]$exts.Add($glob.Substring(1))
        }
        else {
            [void]$names.Add($glob)
        }
    }
    return [pscustomobject]@{ Exts = $exts; Names = $names }
}

function Test-IncludeMatch {
    param([string]$File, $Patterns)
    if ($Patterns.Names.Contains([System.IO.Path]::GetFileName($File))) { return $true }
    return $Patterns.Exts.Contains([System.IO.Path]::GetExtension($File))
}

function Check-Forbidden {
    param(
        [string]$Label,
        [string]$Pattern,
        [string[]]$Include,
        [string]$Reason,
        [switch]$SkipComments,
        # 自检要能直接喂一个临时文件，不必改仓库。默认走全仓库索引。
        [string[]]$FileList,
        # 自检时不想刷屏：只累加失败，不打印「通过」。
        [switch]$Quiet,
        # 自检失败要能被调用方看见，所以默认累加，显式 $false 时只返回命中数。
        [bool]$RecordFailure = $true,
        # 只给自检用：把命中数作为函数输出返回。正常调用不返回，避免把数字打进报告。
        [switch]$PassThru
    )
    $hits = @()
    $patterns = Split-IncludePatterns -Include $Include
    $files = if ($null -ne $FileList) { $FileList } else { Get-SourceIndex }
    foreach ($file in $files) {
        if (-not (Test-IncludeMatch -File $file -Patterns $patterns)) { continue }

        $state = 'code'
        $scanState = [ref]$state
        $lineNo = 0
        foreach ($line in (Get-SourceLines $file)) {
            $lineNo++
            # 绝大多数行没有注释标记，没必要为它们调用一次函数（PowerShell 的函数调用
            # 本身就要几十微秒，几千行累起来就是几百毫秒）。
            $needsMask = $SkipComments -and (
                $state -ne 'code' -or $line.Contains('//') -or $line.Contains('/*') -or $line.Contains('<!--'))
            $scan = if ($needsMask) { Mask-Comments -Text $line -State $scanState } else { $line }
            if ($scan -match $Pattern) {
                $rel = $file.Replace((Get-Location).ProviderPath + '\', '')
                $hits += ("{0}:{1}: {2}" -f $rel, $lineNo, $line.Trim())
            }
        }
    }
    if ($hits.Count -gt 0) {
        if (-not $Quiet) {
            Write-Output "  [禁止] $Label —— $Reason"
            $hits | Select-Object -First 6 | ForEach-Object { Write-Output "         $_" }
            if ($hits.Count -gt 6) { Write-Output ("         ……另有 " + ($hits.Count - 6) + " 处") }
        }
        if ($RecordFailure) { $script:failures += $Label }
    }
    elseif (-not $Quiet) {
        Write-Output "  [通过] $Label"
    }
    if ($PassThru) { Write-Output $hits.Count }
}

# ---------------------------------------------------------------------------
# R75 新增的静态模式。集中定义，既给检查用，也给 -SelfTest 验证。
# ---------------------------------------------------------------------------

# 前端不该有任何网络出口：Slip 是纯本地应用。一旦有 fetch/XHR/WebSocket，
# 用户的数据就可能离开这台机器，而 CSP 里那条 connect-src 'self' 也白配了。
$PATTERN_NETWORK = '\bfetch\s*\(|XMLHttpRequest|new\s+WebSocket|navigator\.sendBeacon|new\s+EventSource'

# 这些 API 能把字符串当代码/标记执行，等价于绕过了"零 HTML 注入"。
$PATTERN_DANGEROUS = '\beval\s*\(|new\s+Function\s*\(|document\.write|\.innerHTML\s*=|\.outerHTML\s*=|insertAdjacentHTML|srcdoc\s*='

# CSP 被削弱（加了 unsafe-* 或整条置空）等于放弃了最后一道防线。
$PATTERN_CSP_WEAK = '"csp"\s*:\s*null|unsafe-inline|unsafe-eval'

Write-Output "== 禁止引入的依赖（AGENTS.md「不可回退的约束」）=="
Check-Forbidden -Label "文件 watcher（notify 等）" `
    -Pattern '^\s*(notify|notify-debouncer-mini|tauri-plugin-fs-watch)\s*=' `
    -Include @('Cargo.toml') `
    -Reason "外部变化只靠写入前读盘比对发现，watcher 会引入第二套时序"

Check-Forbidden -Label "SQLite 及其驱动" `
    -Pattern '^\s*(rusqlite|sqlx|diesel|libsqlite3-sys)\s*=' `
    -Include @('Cargo.toml') `
    -Reason "inbox.md 是唯一真源，索引会造出第二份事实"

Write-Output "== 禁止的代码形态 =="
Check-Forbidden -Label "物理归档目录 archive.md" `
    -Pattern '"archive\.md"|archive\.md' `
    -Include @('*.rs') `
    -Reason "归档只是 marker 上的墓碑，不得转移到第二个文件"

Check-Forbidden -Label "给 presenter 上锁（破坏单队列单消费者）" `
    -Pattern 'Mutex<\s*Presenter\s*>|RwLock<\s*Presenter\s*>' `
    -Include @('*.rs') `
    -Reason "presenter 由消费者独占；上锁意味着出现了第二个写者"

Check-Forbidden -Label "前端 HTML 注入（{@html）" `
    -Pattern '\{@html' `
    -Include @('*.svelte', '*.js') `
    -Reason "inbox.md 是用户可编辑的文件，它的内容不能当 HTML 执行" `
    -SkipComments

Write-Output "== 前端网络与危险 API（R75 新增）=="
Check-Forbidden -Label "前端发起网络请求" `
    -Pattern $PATTERN_NETWORK `
    -Include @('*.svelte', '*.js') `
    -Reason "纯本地应用：允许网络出口等于允许数据外流，也让 CSP 形同虚设" `
    -SkipComments

Check-Forbidden -Label "把字符串当代码/标记执行" `
    -Pattern $PATTERN_DANGEROUS `
    -Include @('*.svelte', '*.js') `
    -Reason "这条与「零 HTML 注入」是同一条约束的不同写法" `
    -SkipComments

Write-Output "== 安全配置（R75 新增）=="
Check-Forbidden -Label "CSP 被削弱" `
    -Pattern $PATTERN_CSP_WEAK `
    -Include @('tauri.conf.json') `
    -Reason "unsafe-inline / unsafe-eval / 置空 csp 都会让注入面重新打开"

if ($SelfTest) {
    Write-Output ""
    Write-Output "== 自检 1：模式对正负样本的判定 =="
    $cases = @(
        @{ Name = '网络请求'; Pattern = $PATTERN_NETWORK; Yes = 'const r = await fetch("http://x")'; No = 'const note = "fetch 还没实现"' }
        @{ Name = '危险 API'; Pattern = $PATTERN_DANGEROUS; Yes = 'el.innerHTML = userText'; No = 'const marker = "innerHTML" /* 说明 */' }
        @{ Name = 'CSP 削弱'; Pattern = $PATTERN_CSP_WEAK; Yes = '"csp": "script-src ''self'' unsafe-inline"'; No = '"csp": "script-src ''self''"' }
    )
    $bad = 0
    foreach ($case in $cases) {
        if ($case.Yes -match $case.Pattern) { Write-Output "  [PASS] $($case.Name)：正样本命中" }
        else { Write-Output "  [FAIL] $($case.Name)：正样本**没有**命中，模式写错了"; $bad++ }
        if ($case.No -notmatch $case.Pattern) { Write-Output "  [PASS] $($case.Name)：负样本不命中" }
        else { Write-Output "  [FAIL] $($case.Name)：负样本被误判"; $bad++ }
    }

    # 只测正则是不够的：上面三条模式全都「对」，但把它们接到 `Cargo.toml` /
    # `tauri.conf.json` 上的 `-Include` 曾经写错，守卫照样恒绿。所以这里走**真实
    # 代码路径**：临时目录造文件 → 调 Check-Forbidden → 断言命中与否。
    Write-Output ""
    Write-Output "== 自检 2：Include 真的能选中 Cargo.toml / tauri.conf.json =="
    $tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("slip-forbidden-selftest-" + [guid]::NewGuid().ToString('n'))
    New-Item -ItemType Directory -Path $tmp | Out-Null
    try {
        $cargoBad = Join-Path $tmp 'Cargo.toml'
        $cargoGood = Join-Path $tmp 'Cargo.good.toml'
        $confBad = Join-Path $tmp 'tauri.conf.json'
        $confGood = Join-Path $tmp 'tauri.good.json'
        Set-Content -LiteralPath $cargoBad -Encoding utf8 -Value @(
            '[dependencies]', 'notify = "6"', 'rusqlite = "0.31"')
        Set-Content -LiteralPath $cargoGood -Encoding utf8 -Value @(
            '[dependencies]', 'serde = "1"')
        Set-Content -LiteralPath $confBad -Encoding utf8 -Value @(
            '{ "app": { "security": { "csp": "script-src ''self'' unsafe-inline" } } }')
        Set-Content -LiteralPath $confGood -Encoding utf8 -Value @(
            '{ "app": { "security": { "csp": "script-src ''self''" } } }')

        $watcher = Check-Forbidden -Label '自检-watcher' -Pattern '^\s*(notify|notify-debouncer-mini)\s*=' `
            -Include @('Cargo.toml') -Reason '自检' -SkipComments -Quiet -RecordFailure $false -PassThru `
            -FileList @($cargoBad, $cargoGood)
        $sqlite = Check-Forbidden -Label '自检-sqlite' -Pattern '^\s*(rusqlite|sqlx|diesel)\s*=' `
            -Include @('Cargo.toml') -Reason '自检' -SkipComments -Quiet -RecordFailure $false -PassThru `
            -FileList @($cargoBad, $cargoGood)
        $csp = Check-Forbidden -Label '自检-csp' -Pattern $PATTERN_CSP_WEAK `
            -Include @('tauri.conf.json') -Reason '自检' -Quiet -RecordFailure $false -PassThru `
            -FileList @($confBad, $confGood)

        foreach ($probe in @(
                @{ Name = 'watcher 命中 Cargo.toml'; Got = $watcher; Want = 1 }
                @{ Name = 'SQLite 命中 Cargo.toml'; Got = $sqlite; Want = 1 }
                @{ Name = 'CSP 命中 tauri.conf.json'; Got = $csp; Want = 1 }
            )) {
            if ($probe.Got -eq $probe.Want) {
                Write-Output "  [PASS] $($probe.Name)：命中 $($probe.Got) 处"
            }
            else {
                Write-Output "  [FAIL] $($probe.Name)：命中 $($probe.Got) 处，期望 $($probe.Want) 处——Include 又选不中文件了"
                $bad++
            }
        }

        # 负样本同样要过：没有违禁项的文件必须 0 命中，否则守卫会天天误报。
        $clean = Check-Forbidden -Label '自检-干净文件' -Pattern $PATTERN_CSP_WEAK `
            -Include @('tauri.conf.json') -Reason '自检' -Quiet -RecordFailure $false -PassThru `
            -FileList @($confGood)
        if ($clean -eq 0) { Write-Output "  [PASS] 干净文件不误报" }
        else { Write-Output "  [FAIL] 干净文件被误报 $clean 处"; $bad++ }
    }
    finally {
        Remove-Item -LiteralPath $tmp -Recurse -Force -ErrorAction SilentlyContinue
    }

    if ($bad -gt 0) { exit 1 }
    Write-Output "自检通过：模式与 Include 都能区分正负样本。"
    # 自检是**加验**，不能掩盖本次全仓扫描的结果。
    if ($failures.Count -gt 0) { exit 1 }
    exit 0
}

Write-Output ""
if ($failures.Count -gt 0) {
    Write-Output ("违反 " + $failures.Count + " 条不可回退的约束：" + ($failures -join '、'))
    Write-Output "这些条目在 AGENTS.md 里有明确理由，改动前先读设计文档对应小节。"
    exit 1
}
Write-Output "全部通过：没有触碰不可回退的约束。"
exit 0
