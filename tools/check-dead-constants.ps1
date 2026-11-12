<#
.SYNOPSIS
    检查"写了但没人用"的常量，以及前后端可能漂移的镜像常量（R128）。

.DESCRIPTION
    两件不同的事：

    1. **死常量**：`export const X = ...` / `pub const X: T = ...` 声明了却没有任何引用。
       这类东西不会报错，只会慢慢让人以为它还在生效。
    2. **镜像漂移**：少数配额常量在两端各有一份（例如"一条最多 2000 个码点"、
       "一次最多拆 100 条"）。前端**不复制**这些数字是设计选择（靠后端拒绝），
       但一旦有人把它们抄进前端并抄错，界面就会在合法输入上误报——所以这里守住"没有第二份"。

    查不出的（常量语义是否仍然正确）不在这里假装覆盖。

.USAGE
    pwsh -NoProfile -File tools/check-dead-constants.ps1
#>
param()

$ErrorActionPreference = 'Stop'
Set-Location (Split-Path -Parent $PSScriptRoot)

$failed = 0

# ---- 1. 前端常量是否被引用 ----
$uiFiles = Get-ChildItem ui/src -Recurse -Include *.js, *.svelte
# **测试也是消费者**：`tools/check-ui-logic.mjs` 直接 import 前端的纯函数与常量，
# 只扫 ui/src 会把"只被测试引用"的常量误报成死代码——而误报会让守卫被关掉，
# 所以这里必须把测试脚本算进引用面。
$consumerFiles = @($uiFiles) + @(Get-Item tools/check-ui-logic.mjs -ErrorAction SilentlyContinue)
$uiText = ($consumerFiles | Where-Object { $_ } | ForEach-Object { Get-Content $_.FullName -Raw }) -join "`n"

$declPattern = 'export const ([A-Z][A-Z0-9_]+)\s*='
$dead = @()
foreach ($file in $uiFiles) {
    $content = Get-Content $file.FullName -Raw
    foreach ($match in [regex]::Matches($content, $declPattern)) {
        $name = $match.Groups[1].Value
        # 在整棵树里数引用次数（声明处本身算一次）
        $count = ([regex]::Matches($uiText, "\b$name\b")).Count
        if ($count -le 1) {
            $dead += "$($file.Name)::$name"
        }
    }
}
if ($dead.Count -gt 0) {
    Write-Output "[FAIL] 前端有 $($dead.Count) 个声明了却没人用的常量："
    $dead | ForEach-Object { Write-Output "         $_" }
    $failed++
}
else {
    Write-Output '[PASS] 前端没有死常量'
}

# ---- 2. Rust 侧常量是否被引用 ----
$rsFiles = Get-ChildItem crates, src-tauri -Recurse -Include *.rs
$rsText = ($rsFiles | ForEach-Object { Get-Content $_.FullName -Raw }) -join "`n"
$rsDead = @()
foreach ($file in $rsFiles) {
    $content = Get-Content $file.FullName -Raw
    foreach ($match in [regex]::Matches($content, 'pub const ([A-Z][A-Z0-9_]+)\s*:')) {
        $name = $match.Groups[1].Value
        # Rust 侧会出现 `#[allow(dead_code)]` 的场景，这里只报"跨文件都没引用"的
        $count = ([regex]::Matches($rsText, "\b$name\b")).Count
        if ($count -le 1) {
            $rsDead += "$($file.Name)::$name"
        }
    }
}
if ($rsDead.Count -gt 0) {
    Write-Output "[WARN] Rust 侧有 $($rsDead.Count) 个 pub const 没有其它引用："
    $rsDead | ForEach-Object { Write-Output "         $_" }
}
else {
    Write-Output '[PASS] Rust 侧没有孤立常量'
}

# ---- 3. 镜像常量不许出现第二份 ----
# 这些数字的**唯一真源**在 Rust 侧；前端出现同值字面量就是重复真相。
$mirrors = @(
    @{ Value = '2000'; Why = 'MAX_TEXT_CODEPOINTS（单条上限）' }
    @{ Value = '100'; Why = 'MAX_LINES（一次拆条上限）' }
)
$uiLiteral = @()
foreach ($file in $uiFiles) {
    $line = 0
    foreach ($row in Get-Content $file.FullName) {
        $line++
        # 只找"看起来像配额判断"的用法：>= / > / <= / < 后跟数字
        foreach ($mirror in $mirrors) {
            if ($row -match "[<>]=?\s*$($mirror.Value)\b") {
                $uiLiteral += "$($file.Name):$line  $($mirror.Why) → $($row.Trim())"
            }
        }
    }
}
if ($uiLiteral.Count -gt 0) {
    Write-Output "[FAIL] 前端出现了配额字面量（这些数字的唯一真源在 Rust 侧）："
    $uiLiteral | ForEach-Object { Write-Output "         $_" }
    $failed++
}
else {
    Write-Output '[PASS] 前端没有配额字面量，两端没有重复真相'
}

if ($failed -gt 0) {
    Write-Output ''
    Write-Output '常量必须要么有人用，要么别写。数字必须只有一个真源。'
    exit 1
}
Write-Output '常量检查通过。'
exit 0
