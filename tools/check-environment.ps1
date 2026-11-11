# 环境自检：这台机器现在能不能跑 Slip 的构建产物与端到端验收？
#
# 起因：本机 Smart App Control 处于强制模式（VerifiedAndReputableDesktop 策略），
# 会静默拒绝执行我们自己编译出来的 exe。症状是 cargo build 通过、cargo test 有时通过，
# 但一启动 slip-shell.exe 就直接失败，端到端 100 项里一大片红。
#
# 这个脚本把当天的排查过程固化下来：先判断"能不能跑"，再说清"不能跑时该怎么办"。
# 它只用 PowerShell（微软签名，自身不受 SAC 影响），不会修改任何系统设置。
param(
    # 真的启动一次产物来验证（默认只在事件日志里查最近的拦截记录，更轻）
    [switch]$Probe,
    [string]$Vault = ''
)

$ErrorActionPreference = 'Continue'
$PSNativeCommandUseErrorActionPreference = $false
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

function Section($title) { Write-Output ""; Write-Output "== $title ==" }

# ---------------------------------------------------------------- 管理员
$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$principal = New-Object Security.Principal.WindowsPrincipal($identity)
$isAdmin = $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
Section "主体"
Write-Output ("管理员: " + $(if ($isAdmin) { "是" } else { "否（CiTool -lp 等查询会失败）" }))

# ---------------------------------------------------------------- SAC
Section "Smart App Control / Code Integrity"
$policy = Get-ItemProperty -Path 'HKLM:\SYSTEM\CurrentControlSet\Control\CI\Policy' -ErrorAction SilentlyContinue
$sac = $policy.VerifiedAndReputablePolicyState
$sacText = switch ($sac) {
    0 { '关闭' }
    1 { '强制（会拦截未签名/无信誉的可执行文件）' }
    2 { '评估（只记录，不拦截）' }
    default { "未知（$sac）" }
}
Write-Output "VerifiedAndReputablePolicyState = $sac  → $sacText"

try {
    $guard = Get-CimInstance -ClassName Win32_DeviceGuard `
        -Namespace root\Microsoft\Windows\DeviceGuard -ErrorAction Stop
    $enforce = switch ($guard.CodeIntegrityPolicyEnforcementStatus) {
        0 { '未启用' }
        1 { '审计' }
        2 { '强制' }
        default { "未知（$($guard.CodeIntegrityPolicyEnforcementStatus)）" }
    }
    Write-Output "CodeIntegrityPolicyEnforcementStatus = $($guard.CodeIntegrityPolicyEnforcementStatus) → $enforce"
} catch {
    Write-Output "DeviceGuard 状态查询失败：$($_.Exception.Message)"
}

# ---------------------------------------------------------------- 生效的策略
$activeDir = 'C:\Windows\System32\CodeIntegrity\CiPolicies\Active'
if (Test-Path $activeDir) {
    $count = (Get-ChildItem $activeDir -Filter *.cip -ErrorAction SilentlyContinue | Measure-Object).Count
    Write-Output "CiPolicies\Active 里的策略文件: $count 个"
}
if ($isAdmin -and (Test-Path 'C:\Windows\System32\CiTool.exe')) {
    Write-Output "-- 生效中的策略（CiTool -lp）--"
    & CiTool.exe -lp 2>&1 | Select-String -Pattern '好记的名称|当前强制执行|策略 ID' |
        ForEach-Object { '   ' + $_.Line.Trim() }
} else {
    Write-Output '（要看完整策略清单，用管理员运行：CiTool.exe -lp）'
}

# ---------------------------------------------------------------- 产物运行性
Section "构建产物"
$exe = Join-Path $root 'target\debug\slip-shell.exe'
if (-not (Test-Path $exe)) {
    Write-Output "slip-shell.exe 不存在 —— 先跑 cargo build -p slip-shell"
    $exeBlocked = $true
} else {
    Write-Output ("slip-shell.exe 构建于 " + (Get-Item $exe).LastWriteTime)

    # 事件日志里的拦截记录：判断"这个文件最近有没有被 CI 拒绝过"
    $logName = 'Microsoft-Windows-CodeIntegrity/Operational'
    $blockedRecently = $false
    try {
        $events = Get-WinEvent -LogName $logName -MaxEvents 200 -ErrorAction Stop |
            Where-Object { $_.Id -in 3077, 3033 } |
            Where-Object { $_.Message -like '*slip-shell.exe*' } |
            Select-Object -First 1
        if ($events) {
            $blockedRecently = $true
            Write-Output ("最近一条拦截记录: " + $events.TimeCreated)
            Write-Output ("   " + (($events.Message -replace '\s+', ' ').Trim()))
        } else {
            Write-Output '事件日志里没有该产物的拦截记录'
        }
    } catch {
        Write-Output "事件日志读取失败（通常需要管理员）：$($_.Exception.Message)"
    }

    if ($Probe) {
        Get-Process slip-shell -ErrorAction SilentlyContinue | Stop-Process -Force
        Start-Sleep -Milliseconds 400
        try {
            $proc = Start-Process $exe -PassThru -ErrorAction Stop
            Start-Sleep -Seconds 3
            $alive = -not $proc.HasExited
            Get-Process slip-shell -ErrorAction SilentlyContinue | Stop-Process -Force
            Write-Output ("实测启动: " + $(if ($alive) { '成功' } else { '进程立刻退出' }))
            if (-not $alive) { $blockedRecently = $true }
        } catch {
            Write-Output '实测启动: 被系统拒绝（与事件日志的拦截记录一致）'
            $blockedRecently = $true
        }
    } else {
        Write-Output '（想实测启动一次，加 -Probe）'
    }
    $exeBlocked = $blockedRecently
}

# ---------------------------------------------------------------- 处置建议
Section "结论"
if (-not $exeBlocked) {
    Write-Output '构建产物可以运行 —— 端到端验收可以照常跑：'
    Write-Output '   pwsh -File tools/verify-hotkey.ps1'
    exit 0
}

Write-Output '构建产物被系统拒绝。这意味着：'
Write-Output '   可以: cargo fmt / clippy / build / test（测试二进制多数时候能跑）'
Write-Output '   不行: 启动 slip-shell.exe、端到端验收（verify-hotkey.ps1）'
Write-Output ''
Write-Output '能解决的三条路（按推荐度）：'
Write-Output '   1) 关闭 Smart App Control：设置 → 隐私和安全性 → Windows 安全中心 →'
Write-Output '      应用和浏览器控制 → 智能应用控制 → 关闭。注意这是单向开关，'
Write-Output '      关闭后要再打开需要重置系统；部分机器该选项是灰的。'
Write-Output '   2) 换一个没有 SAC 的 Windows 环境：Windows Sandbox（默认不带 SAC）'
Write-Output '      或 Hyper-V 虚拟机，把仓库拷进去跑同一套验收脚本。'
Write-Output '   3) 换一台机器。'
Write-Output ''
Write-Output '不管用的做法（别浪费时间）：'
Write-Output '   - 自签名证书 / signtool 签名：SAC 看云端信誉，不看本地证书链'
Write-Output '   - 加补充 WDAC 策略：SAC 是签过名的平台策略，不接受用户补充策略'
Write-Output '   - Defender 排除目录、关实时保护、改 SmartScreen：都不是代码完整性'
Write-Output '   - 换目录 / 改文件名：拦截按文件内容，与路径无关'
exit 1
