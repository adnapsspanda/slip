# 解析仓库里所有 *.ps1，报告语法错误。
#
# 为什么单独成文件：编码检查只管 BOM 不管语法，而本仓库的验收脚本被频繁手改
# （引号、here-string、中文注释都踩过坑），语法错却要等到有人在本机跑那条命令
# 才会暴露。让 CI 与提交钩子共用这一份实现，避免两边各写一套解析逻辑。
#
# 用法：pwsh -File tools/check-ps1-syntax.ps1

$ErrorActionPreference = "Stop"
$root = Split-Path $PSScriptRoot -Parent

$bad = @()
Get-ChildItem -Path $root -Recurse -Filter *.ps1 -File -ErrorAction SilentlyContinue |
    ForEach-Object {
        $errors = $null
        [void][System.Management.Automation.Language.Parser]::ParseFile(
            $_.FullName, [ref]$null, [ref]$errors)
        if ($errors -and $errors.Count -gt 0) {
            $first = $errors[0]
            $bad += ("{0}: {1}（第 {2} 行）" -f $_.Name, $first.Message, $first.Extent.StartLineNumber)
        }
    }

if ($bad.Count -gt 0) {
    Write-Output "以下 .ps1 有语法错误："
    $bad | ForEach-Object { Write-Output "  $_" }
    exit 1
}

Write-Output "全部 .ps1 语法通过"
