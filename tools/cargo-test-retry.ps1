# 在应用控制策略（Smart App Control）阻止新产物的环境里跑 cargo test。
#
# 症状：`cargo test` 报 `应用程序控制策略已阻止此文件。 (os error 4551)`，
# 被点名的是 target/debug/deps 下某个刚编出来的测试二进制。同一个文件删掉重编后
# 往往就能跑（策略对新建文件的放行有延迟/间歇性）。
#
# 本脚本**只对这一种错误重试**：命中时把被点名的 exe 删掉再跑一轮，最多 5 次。
# 任何其它测试失败都原样上抛，不重试、不掩盖。
param(
    [int]$MaxAttempts = 5,
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

    foreach ($match in $blocked) {
        $path = $match.Groups[1].Value
        if (Test-Path $path) {
            Remove-Item $path -Force
            Write-Output "[retry] 第 $attempt 次：删除被策略阻止的产物 $(Split-Path -Leaf $path)"
        }
    }
    Start-Sleep -Seconds 2
}

exit 1
