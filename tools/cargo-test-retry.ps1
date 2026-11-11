# 在应用控制策略（Smart App Control）阻止新产物的环境里跑 cargo test。
#
# 症状：`cargo test` 报 `应用程序控制策略已阻止此文件。 (os error 4551)`，
# 被点名的是 target/debug/deps 下某个刚编出来的测试二进制。
#
# **实测出来的规律（2026-11-11）**：拦截是**按产物**的，与谁启动它无关。
# 一次 `cargo run` 编出的 slip-shell.exe 在 17:45 被拒，17:57 同一份文件却能跑通。
# 所以正确策略是**对同一个二进制重试**；反过来，删掉它去重编是**帮倒忙**——
# 每次重编都是一个新产物，又要重新经历"首次执行被拒"。
#
# 本脚本只对 `os error 4551` 重试，且默认**不删产物**；连续失败超过阈值后
# 才把它当成"这个哈希被永久记住"，删一次并给出提示。其它任何失败原样上抛。
param(
    [int]$MaxAttempts = 6,
    # 连续被拦多少次之后才认定"这个产物被永久记住"并删掉重建
    [int]$DeleteAfter = 4,
    [string[]]$CargoArgs = @('test', '--workspace', '--locked', '--offline')
)

$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $false
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

for ($attempt = 1; $attempt -le $MaxAttempts; $attempt++) {
    $output = & cargo @CargoArgs 2>&1 | Out-String
    $code = $LASTEXITCODE

    if ($code -eq 0) {
        Write-Output $output
        if ($attempt -gt 1) {
            Write-Output "[retry] 第 $attempt 次尝试通过（前几次是应用控制策略拦截产物，非代码问题）"
        }
        exit 0
    }

    $blocked = [regex]::Matches($output, 'could not execute process `([^`]+\.exe)`')
    $isPolicyBlock = $output -match 'os error 4551'
    if (-not $isPolicyBlock -or $blocked.Count -eq 0 -or $attempt -eq $MaxAttempts) {
        Write-Output $output
        Write-Output "[retry] 第 $attempt 次失败且不是可重试的产物拦截，原样返回退出码 $code"
        exit $code
    }

    # 默认只等一等再跑同一个二进制：实测同一份文件重试几次就能过。
    if ($attempt -ge $DeleteAfter) {
        foreach ($match in $blocked) {
            $path = $match.Groups[1].Value
            if (Test-Path $path) {
                Remove-Item $path -Force
                Write-Output "[retry] 连续 $attempt 次被拦，认定该产物被永久记住，删除重建：$(Split-Path -Leaf $path)"
            }
        }
    } else {
        Write-Output "[retry] 第 $attempt 次被拦，等待后重试同一个二进制（不删产物）"
    }
    Start-Sleep -Seconds 3
}

exit 1
