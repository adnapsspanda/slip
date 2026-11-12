<#
.SYNOPSIS
    检查设计文档里的**具体数字与清单**是否与代码一致（R78）。

.DESCRIPTION
    设计文档写"7 个 IPC 命令"、代码里已经有 21 个——这类失真最难发现，因为文档不会报错，
    而读到它的人会按错的数字去理解系统。

    这里只查**能被机器判定**的部分：
    - 文档里的「N 个 IPC 命令」与 `#[tauri::command]` 的实际数量；
    - 文档里的 capability 清单与 `capabilities/default.json` 的实际 permissions；
    - 文档里出现 "spike 面板 / spike_* 命令" 这类已被删除的东西（rev 14 已声明删除）。

    查不出的（架构叙述是否仍然准确）不在这里假装覆盖。

.USAGE
    pwsh -NoProfile -File tools/check-doc-sync.ps1
#>
param()

$ErrorActionPreference = 'Stop'
Set-Location (Split-Path -Parent $PSScriptRoot)

$doc = 'docs/design/Slip-架构与模块定义.md'
if (-not (Test-Path $doc)) {
    Write-Output "[FAIL] 找不到 $doc"
    exit 1
}

$failed = 0
# **剥掉 BOM 字符**：`Get-Content -Raw` 在 Windows PowerShell 5.1 下会把 U+FEFF
# 留在字符串开头，`ConvertFrom-Json` 于是抛 ArgumentException——而且退出码是 1，
# 看起来像文档不一致，实际是编码问题。设计文档与 capability JSON 都可能带 BOM。
function Read-Text([string]$Path) {
    return ((Get-Content -LiteralPath $Path -Raw) -replace "^\uFEFF", '')
}
$text = Read-Text $doc
$docLines = (Read-Text $doc) -split "`r?`n"

# ---- 1. IPC 命令数量 ----
$actualCommands = (Select-String -Path 'src-tauri/src/*.rs' -Pattern '#\[tauri::command\]' | Measure-Object).Count
# 只看**规范性叙述**：修订表里会出现"写着「7 个 IPC 命令」"这种历史描述，
# 它不是断言，扫进来就是误报。修订表行形如 `| 23 | 修正 | …`。
$normative = ($docLines | Where-Object { $_ -notmatch '^\|\s*\d+\s*\|' }) -join "`n"
$claims = [regex]::Matches($normative, '(\d+)\s*个\s*IPC\s*命令')
if ($claims.Count -eq 0) {
    Write-Output '[WARN] 文档里没有「N 个 IPC 命令」的断言，无法核对命令数'
}
else {
    $bad = @()
    foreach ($m in $claims) {
        $claimed = [int]$m.Groups[1].Value
        if ($claimed -ne $actualCommands) { $bad += $claimed }
    }
    if ($bad.Count -gt 0) {
        Write-Output "[FAIL] 文档写「$($bad -join '、') 个 IPC 命令」，实际 $actualCommands 个"
        $failed++
    }
    else {
        Write-Output "[PASS] IPC 命令数一致（$actualCommands）"
    }
}

# ---- 2. capability 清单 ----
# 只读文档里那句**声明**（"Capability 只有两项：…"），不扫全文——
# 全文里还有"core:window:default 的组成是…"这类解释，扫全文会把解释当成权限。
$capPath = 'src-tauri/capabilities/default.json'
$cap = Read-Text $capPath | ConvertFrom-Json
$actualPerms = @($cap.permissions | Sort-Object)

$declareIndex = -1
for ($i = 0; $i -lt $docLines.Count; $i++) {
    if ($docLines[$i] -match 'Capability\s*(只有|是)') { $declareIndex = $i; break }
}
if ($declareIndex -lt 0) {
    Write-Output '[WARN] 文档里没有 capability 的声明句，无法核对'
}
else {
    # 声明句可能跨几行（列表形式），取这一行及之后 4 行。
    $chunk = ($docLines[$declareIndex..([Math]::Min($declareIndex + 4, $docLines.Count - 1))] -join "`n")
    $docPerms = [regex]::Matches($chunk, '`((?:core|clipboard-manager|notification)[a-z:._-]*?(?:allow-[a-z-]+|:default))`') |
        ForEach-Object { $_.Groups[1].Value } | Sort-Object -Unique
    $missingInDoc = @($actualPerms | Where-Object { $docPerms -notcontains $_ })
    $extraInDoc = @($docPerms | Where-Object { $actualPerms -notcontains $_ })
    if ($missingInDoc.Count -gt 0) {
        Write-Output "[FAIL] 代码里有、文档声明没提的权限：$($missingInDoc -join '、')"
        $failed++
    }
    if ($extraInDoc.Count -gt 0) {
        Write-Output "[FAIL] 文档声明提到、代码里没有的权限：$($extraInDoc -join '、')"
        $failed++
    }
    if ($missingInDoc.Count -eq 0 -and $extraInDoc.Count -eq 0) {
        Write-Output "[PASS] capability 声明一致（$($actualPerms -join '、')）"
    }
}

# ---- 3. 已删除的东西不该再被当成现存功能描述 ----
# rev 14 声明 `spike_*` 命令集已删除；rev 15 声明视图通路整块删除。
# 文档里仍会保留历史叙述（这是对的），但**不该再以"现行功能"的口吻出现**。
$staleMarkers = @(
    @{ Pattern = 'spike 面板'; Why = 'rev 14 已删除 spike 面板' }
    @{ Pattern = 'ViewEvent 四变体'; Why = 'rev 15 已整块删除视图通路' }
    @{ Pattern = 'ToggleCollapse'; Why = 'rev 15 已删除折叠开关' }
)
foreach ($marker in $staleMarkers) {
    $hits = Select-String -Path $doc -Pattern ([regex]::Escape($marker.Pattern))
    if ($hits) {
        # 历史语境允许出现——那里正是在说明"删掉了"。仅靠行号判断不够准
        # （§-section 里也可能有一句"已随修订 15 删除"），所以按**语义线索**过滤。
        # 这条规则宁可漏报也不制造噪音：一条天天误报的 WARN 会让人学会无视整个脚本。
        $outsideHistory = $hits | Where-Object {
            $_.Line -notmatch '删|修订|已随|将来|曾经|历史'
        }
        if ($outsideHistory) {
            Write-Output "[WARN] 正文里仍有「$($marker.Pattern)」——$($marker.Why)，确认它出现在历史语境里"
        }
    }
}

# ---- 4. 前端预算数字：脚本、文档、钩子注释三处必须一致 ----
#
# 这类数字最典型的失真方式：预算从 70 KB 提到 88 KB，脚本改了，文档与钩子注释
# 还写着旧值。读者按错的数字理解系统，而没有任何东西会报错。
$bundleScript = 'tools/check-bundle-size.ps1'
$budgetSources = @('docs/handover.md', '.githooks/pre-commit', 'ui/src/lib/tags.js')
if (Test-Path $bundleScript) {
    $bundleText = Read-Text $bundleScript
    $cssMatch = [regex]::Match($bundleText, '\$cssLimit\s*=\s*(\d+)\s*\*\s*1024')
    $jsMatch = [regex]::Match($bundleText, '\$jsLimit\s*=\s*(\d+)\s*\*\s*1024')
    if (-not $cssMatch.Success -or -not $jsMatch.Success) {
        Write-Output '[FAIL] 无法从 check-bundle-size.ps1 解析出 css/js 预算，断言失效'
        $failed++
    }
    else {
        $cssKb = [int]$cssMatch.Groups[1].Value
        $jsKb = [int]$jsMatch.Groups[1].Value
        foreach ($source in $budgetSources) {
            if (-not (Test-Path $source)) { continue }
            $body = Read-Text $source
            # 只看提到 KB 预算的句子，避免把「R84 时产物 71.09 KB」这类历史数字扫进来。
            $claims = [regex]::Matches($body, '(CSS|JS)[^。\n]{0,24}?(\d+)\s*KB')
            foreach ($claim in $claims) {
                $kind = $claim.Groups[1].Value
                $claimed = [int]$claim.Groups[2].Value
                $expected = if ($kind -eq 'CSS') { $cssKb } else { $jsKb }
                if ($claimed -ne $expected) {
                    Write-Output "[FAIL] $source 写「$kind $claimed KB」，脚本预算是 $kind $expected KB"
                    $failed++
                }
            }
        }
        if ($failed -eq 0) {
            Write-Output "[PASS] 预算数字一致（CSS $cssKb KB / JS $jsKb KB）"
        }
    }
}

if ($failed -gt 0) {
    Write-Output ''
    Write-Output '设计文档是唯一真源：文档与代码冲突时先改文档。但**数字要对得上**。'
    exit 1
}
Write-Output '设计文档的机器可查部分与代码一致。'
exit 0
