<#
.SYNOPSIS
    打绿色便携包（R80）：编译 release 二进制 → 校验产物 → 打 zip。

.DESCRIPTION
    Slip 是单 exe 绿色版：解压即可运行，不装、不写注册表（除开机自启那一项，且是用户显式勾选
    才写）。所以发布流程要保证三件事：

    1. **前端产物先进二进制**：`ui/dist` 是编译期被 `tauri::generate_context!` 嵌进去的，
       顺序错了就会打进旧界面（且不会有任何报错）；
    2. **产物可运行**：体积合理、PE 头正常、能取到版本信息；
    3. **签名状态如实报告**：本机没有代码签名证书，产物是**未签名**的。
       这不影响运行，但 Windows Smart App Control 会对未签名程序按信誉判定（本机实测会拦新编出
       的 exe）。脚本里做成"报告 + 说明"，不假装签过。

.USAGE
    pwsh -NoProfile -File tools/package-release.ps1
    pwsh -NoProfile -File tools/package-release.ps1 -SkipBuild   # 用现有产物直接打包
#>
param(
    [switch]$SkipBuild,
    [string]$OutDir = 'dist-release',
    # 覆盖输入二进制。**只用于在 release 构建不可用时验证打包流程本身**
    # （本机 Smart App Control 会拦 release 模式下的 proc-macro DLL）。
    # 正常发布不要传这个参数。
    [string]$ExePath = 'target\release\slip-shell.exe'
)

$ErrorActionPreference = 'Stop'
Set-Location (Split-Path -Parent $PSScriptRoot)

$bun = 'C:\Users\spand\.bun\bin\bun.exe'
$exe = $ExePath
if ($ExePath -ne 'target\release\slip-shell.exe') {
    Write-Output "[注意] 输入二进制被覆盖为 $ExePath —— 这不是正式发布产物，仅用于验证打包流程"
}
$failed = 0

function Step([string]$Text) { Write-Output "`n== $Text ==" }

if (-not $SkipBuild) {
    Step '构建前端产物'
    Push-Location ui
    try {
        & $bun run build | Out-Null
        if ($LASTEXITCODE -ne 0) { throw '前端构建失败' }
    }
    finally { Pop-Location }

    Step '编译 release 二进制（首次可能要几分钟）'
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    cargo build -p slip-shell --release --locked
    if ($LASTEXITCODE -ne 0) { throw 'release 构建失败' }
    $sw.Stop()
    Write-Output ("  构建耗时 {0:N1}s" -f $sw.Elapsed.TotalSeconds)
}

if (-not (Test-Path $exe)) {
    Write-Output "[FAIL] 找不到 $exe"
    exit 1
}

Step '校验产物'
$item = Get-Item $exe
$sizeMb = [Math]::Round($item.Length / 1MB, 2)
Write-Output "  路径：$exe"
Write-Output "  体积：$sizeMb MB"
Write-Output "  时间：$($item.LastWriteTime)"

# 体积守卫：超过 25 MB 说明有东西被误打进去了（正常在 10–15 MB）。
if ($sizeMb -gt 25) {
    Write-Output "  [FAIL] 体积异常（$sizeMb MB），确认是否误打了调试信息或资源"
    $failed++
}
else {
    Write-Output '  [PASS] 体积在合理区间'
}

# 前端产物必须比源码新，否则打进去的是旧界面（这类错误不会有任何报错）。
$distIndex = 'ui/dist/index.html'
if (Test-Path $distIndex) {
    $newer = Get-ChildItem ui/src -Recurse -File |
        Where-Object { $_.LastWriteTime -gt (Get-Item $distIndex).LastWriteTime } |
        Select-Object -First 1
    if ($newer) {
        Write-Output "  [WARN] 前端源码比产物新（$($newer.Name)），先跑一次前端构建"
    }
}

# 版本信息能取到，说明 PE 资源没坏。
$version = (Get-Item $exe).VersionInfo
Write-Output "  版本：$($version.FileVersion) / 产品：$($version.ProductName)"

# 签名：如实报告，不假装。
$sig = Get-AuthenticodeSignature $exe
Write-Output "  签名状态：$($sig.Status)"
if ($sig.Status -ne 'Valid') {
    Write-Output '    （本机没有代码签名证书。未签名不影响运行，但 Smart App Control 会按信誉判定——'
    Write-Output '      本机实测新编出的未签名 exe 时好时坏，见 AGENTS.md 的环境陷阱）'
}

Step '打包'
if (-not (Test-Path $OutDir)) { New-Item -ItemType Directory -Path $OutDir | Out-Null }
$stamp = Get-Date -Format 'yyyyMMdd-HHmm'
$zipPath = Join-Path $OutDir "slip-$stamp.zip"

# 包里只放必要文件：exe + 一份给人看的说明。
$staging = Join-Path $env:TEMP ("slip-pack-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $staging | Out-Null
Copy-Item $exe (Join-Path $staging 'slip-shell.exe')

$readme = @"
Slip —— 极简便签待办

用法
  1. 双击 slip-shell.exe 即可运行（绿色版，不写注册表）。
  2. 默认热键 Ctrl+Alt+Space 唤出；托盘图标右键有菜单。
  3. 数据保存在：%USERPROFILE%\Documents\Slip\inbox.md
     这是**纯文本**文件，你可以直接用任何编辑器打开、备份、或纳入 git。

如果双击后没有窗口
  - 请先确认任务管理器里有没有 slip-shell.exe；
  - Windows Smart App Control 可能拦截未签名程序（本包未做代码签名）。
    在「Windows 安全中心 → 应用和浏览器控制 → 智能应用控制」里可查看状态。
  - 可用 `SLIP_LOG=debug` 环境变量启动，日志会打到 stderr。

系统要求
  Windows 11 + WebView2 运行时（Win11 自带）。
"@
Set-Content -Path (Join-Path $staging 'README.txt') -Value $readme -Encoding utf8

$zipInput = Get-ChildItem $staging | Select-Object -ExpandProperty FullName
Compress-Archive -Path $zipInput -DestinationPath $zipPath -CompressionLevel Optimal -Force
Remove-Item $staging -Recurse -Force

$zipItem = Get-Item $zipPath
Write-Output ("  {0}（{1:N2} MB）" -f $zipItem.Name, ($zipItem.Length / 1MB))
Write-Output "  位置：$($zipItem.FullName)"

# 解压回来核对一次：打包成功但内容缺失是很常见的静默失败。
$verifyDir = Join-Path $env:TEMP ("slip-verify-" + [guid]::NewGuid().ToString('N'))
Expand-Archive -Path $zipPath -DestinationPath $verifyDir -Force
$entries = Get-ChildItem $verifyDir | Select-Object -ExpandProperty Name
Remove-Item $verifyDir -Recurse -Force
Write-Output "  包内文件：$($entries -join '、')"
if ($entries -notcontains 'slip-shell.exe') {
    Write-Output '  [FAIL] 包里没有 slip-shell.exe'
    $failed++
}
else {
    Write-Output '  [PASS] 包内包含 slip-shell.exe'
}

Write-Output ''
if ($failed -gt 0) {
    Write-Output "打包完成，但有 $failed 项需要处理。"
    exit 1
}
Write-Output '发布包就绪。'
exit 0
