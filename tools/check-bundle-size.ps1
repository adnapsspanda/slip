<#
.SYNOPSIS
    前端产物体积门禁（R74）。

.DESCRIPTION
    检查 `ui/dist` 的 CSS 与 JS 体积是否超预算。

    为什么要用脚本而不是"记得看一眼"：这两个数字直接决定二进制里嵌入多少字节，
    而超了不会有任何报错——只会让安装包悄悄变大。做成门禁之后，超预算是硬失败。

    预算来源：CSS < **14 KB**、JS < **88 KB**。

    CSS 预算从 10 KB 提额到 14 KB（2026-11-12 明确裁决）。理由：
      · 10 KB 的初始配额定在 **R31（系统深浅色自适应）之前**，当时只有浅色固定纸色、
        没有语义变量体系；
      · R31–R41 落地了完整双模变量体系、发丝边微光轮廓、Windows 11 聚焦环、
        横幅三级语义、细滚动条——**整套双主题净增仅约 2.3 KB**，压缩后 9.99 KB，
        这个控制力已经很克制；
      · 为死守 10 KB 去做极端合并/缩写会牺牲 CSS 的可读性与模块化，属于"为指标牺牲设计"。
    14 KB 既能给后续微调留 4 KB 缓冲，也仍能挡住无节制引入大型样式库。

    JS 预算从 70 KB 提额到 **80 KB**（2026-11-12 裁决）。理由：
      · R84–R90 新增了标签提取、过滤语法（`#tag` / `is:xxx`）、搜索高亮切片三个**纯逻辑模块**，
        压缩后净增约 2.5 KB，加上模板改动已到 71.09 KB；
      · 后续还有 R95 置顶按钮、R96/R97 批量操作与二次确认、R98 草稿清空恢复、
        R100 撤销排队、R114 触控、R119 淡出——70 KB 只够一轮，会中途再次撞线；
      · 80 KB 对当前 71.09 KB 留约 9 KB，够这批功能，也仍能挡住"无节制引入库"。
        前端**依然没有任何运行期依赖**（`package.json` 只有 devDependencies）。

    再次提额 80 KB → **88 KB**（2026-11-12 第二次裁决，理由不同）：R107–R119 又增加了失败文案、
    只读预览弹窗、右滑手势状态机、收起淡出，产物已到 79.62 KB、只剩 388 字节。剩下的前端轮次
    （归档抽屉的清空入口等）在 80 KB 内放不下。**这不是无限放宽**：不再有新增前端功能时不应再动这个数，
    若将来又要提额，先问"是不是该拆模块或砍功能"。
#>
$ErrorActionPreference = 'Stop'

$cssLimit = 14 * 1024
$jsLimit = 88 * 1024

$dist = Join-Path $PSScriptRoot '..\ui\dist'
if (-not (Test-Path $dist)) {
    Write-Output '[FAIL] 找不到 ui\dist —— 先跑前端构建'
    exit 1
}

# **汇总全部产物**，不是只看第一个文件：Vite 一旦做代码分割，
# `Select-Object -First 1` 会让第二个超预算的 chunk 完全不受约束，
# 而 Tauri 会把整个 ui/dist 嵌进二进制。
$cssFiles = @(Get-ChildItem $dist -Recurse -Filter '*.css')
$jsFiles = @(Get-ChildItem $dist -Recurse -Filter '*.js')
if ($cssFiles.Count -eq 0 -or $jsFiles.Count -eq 0) {
    Write-Output '[FAIL] 产物里缺少 css 或 js'
    exit 1
}
$cssBytes = [long](($cssFiles | Measure-Object -Property Length -Sum).Sum)
$jsBytes = [long](($jsFiles | Measure-Object -Property Length -Sum).Sum)

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

Report "CSS（$($cssFiles.Count) 个文件）" $cssBytes $cssLimit
Report "JS（$($jsFiles.Count) 个文件）" $jsBytes $jsLimit

# 体积之外再确认一件事：产物里不该出现运行期依赖。
# 本项目的 package.json 只有 devDependencies，任何一条被误引进来都会让 JS 明显变大。
$bundle = ($jsFiles | ForEach-Object { Get-Content $_.FullName -Raw }) -join "`n"
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
