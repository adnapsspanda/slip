<#
.SYNOPSIS
    前端产物体积门禁（R74）。

.DESCRIPTION
    检查 `ui/dist` 的 CSS 与 JS 体积是否超预算。

    为什么要用脚本而不是"记得看一眼"：这两个数字直接决定二进制里嵌入多少字节，
    而超了不会有任何报错——只会让安装包悄悄变大。做成门禁之后，超预算是硬失败。

    预算来源：矩阵 R74（CSS < 10 KB、JS < 70 KB）。当前实测 CSS 9.99 KB / JS 68.04 KB，
    **CSS 只剩约 10 字节余量**——这不是健康状态，见下面的"当前余量"输出。
#>
$ErrorActionPreference = 'Stop'

$cssLimit = 10 * 1024
$jsLimit = 70 * 1024

$dist = Join-Path $PSScriptRoot '..\ui\dist'
if (-not (Test-Path $dist)) {
    Write-Output '[FAIL] 找不到 ui\dist —— 先跑前端构建'
    exit 1
}

$css = Get-ChildItem $dist -Recurse -Filter '*.css' | Select-Object -First 1
$js = Get-ChildItem $dist -Recurse -Filter '*.js' | Select-Object -First 1
if (-not $css -or -not $js) {
    Write-Output '[FAIL] 产物里缺少 css 或 js'
    exit 1
}

$failed = 0
function Report([string]$Label, [long]$Actual, [long]$Limit) {
    $kb = [Math]::Round($Actual / 1KB, 2)
    $limitKb = [Math]::Round($Limit / 1KB, 0)
    $margin = $Limit - $Actual
    if ($Actual -gt $Limit) {
        Write-Output "[FAIL] $Label $kb KB 超过预算 $limitKb KB（多 $([Math]::Round($margin / 1KB, 2)) KB）"
        $script:failed++
    }
    elseif ($margin -lt 512) {
        # 不到 0.5 KB 余量：现在通过，但下一次改动很容易撞线，值得提醒。
        Write-Output "[WARN] $Label $kb KB / $limitKb KB —— 只剩 $margin 字节余量"
    }
    else {
        Write-Output "[PASS] $Label $kb KB / $limitKb KB（余 $([Math]::Round($margin / 1KB, 2)) KB）"
    }
}

Report 'CSS' $css.Length $cssLimit
Report 'JS' $js.Length $jsLimit

# 体积之外再确认一件事：产物里不该出现运行期依赖。
# 本项目的 package.json 只有 devDependencies，任何一条被误引进来都会让 JS 明显变大。
$bundle = Get-Content $js.FullName -Raw
$suspects = @('lodash', 'moment', 'rxjs', 'jquery', 'axios', 'marked', 'dompurify')
$found = $suspects | Where-Object { $bundle -match [regex]::Escape($_) }
if ($found) {
    Write-Output "[FAIL] 产物里出现了疑似第三方库：$($found -join ', ')"
    $failed++
}
else {
    Write-Output '[PASS] 产物里没有常见第三方库（无冗余依赖）'
}

if ($failed -gt 0) { exit 1 }
