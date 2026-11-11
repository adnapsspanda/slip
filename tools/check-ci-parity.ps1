<#
.SYNOPSIS
    检查提交钩子与 CI 的门禁是否**零漂移**（R76）。

.DESCRIPTION
    本地全绿、CI 变红（或者反过来）是最常见的一类"门禁不可信"。两者的步骤列表
    很容易在改动中慢慢分叉：某人在钩子里加了检查忘了同步 CI，或者 CI 加了钩子没有。

    这个脚本把两边的步骤抽出来，按**关键词**归一到统一的关口名，然后比对集合差。

    刻意做成"归一 + 白名单"而不是逐字比对：两边的措辞本来就不需要完全一样
    （钩子的 `cargo fmt --all --check` 与 CI 的 `格式检查` 是同一件事），
    逐字比只会制造噪音，噪音会让守卫被关掉。

.USAGE
    pwsh -NoProfile -File tools/check-ci-parity.ps1
#>
param()

$ErrorActionPreference = 'Stop'
Set-Location (Split-Path -Parent $PSScriptRoot)

$hookPath = '.githooks/pre-commit'
$ciPath = '.github/workflows/ci.yml'
foreach ($path in @($hookPath, $ciPath)) {
    if (-not (Test-Path $path)) {
        Write-Output "[FAIL] 找不到 $path"
        exit 1
    }
}

# 关口定义：关键词 → 规范名。关键词按顺序匹配，第一个命中的生效。
$gates = @(
    @{ Key = 'BOM'; Name = '编码（BOM）'; Hook = 'BOM'; Ci = 'BOM' }
    @{ Key = '语法||syntax'; Name = 'PowerShell 语法'; Hook = 'PowerShell 语法检查'; Ci = 'PowerShell 语法检查' }
    @{ Key = '不可回退||forbidden'; Name = '不可回退约束'; Hook = '不可回退'; Ci = '不可回退' }
    @{ Key = '纯逻辑||ui-logic'; Name = '前端纯逻辑'; Hook = '前端纯逻辑'; Ci = '前端纯逻辑' }
    @{ Key = '产物体积||bundle'; Name = '前端产物体积'; Hook = '前端产物体积'; Ci = '前端产物体积' }
    # 注意顺序：`前端格式` 必须排在通用的 `格式检查` 之前，否则 CI 的
    # 「前端格式检查」会被先命中的 `格式检查` 抢走、误判成 Rust 格式。
    @{ Key = 'prettier||前端格式'; Name = '前端格式'; Hook = 'prettier'; Ci = '前端格式检查' }
    @{ Key = '前端产物||构建前端'; Name = '前端构建'; Hook = '先构建'; Ci = '构建前端产物' }
    @{ Key = 'cargo fmt||格式检查'; Name = 'Rust 格式'; Hook = 'cargo fmt'; Ci = '格式检查' }
    @{ Key = 'clippy'; Name = 'Clippy'; Hook = 'clippy'; Ci = 'Clippy' }
    @{ Key = 'cargo test||测试'; Name = '测试'; Hook = 'cargo test'; Ci = '测试' }
)

function Canonicalize([string]$Text) {
    foreach ($gate in $gates) {
        foreach ($kw in $gate.Key.Split('||')) {
            if ($Text -match [regex]::Escape($kw)) { return $gate.Name }
        }
    }
    return $null
}

# 钩子：形如 `step "编码检查（*.ps1 必须有 BOM）"`
$hookSteps = Select-String -Path $hookPath -Pattern '^\s*step\s+"([^"]+)"' |
    ForEach-Object { $_.Matches[0].Groups[1].Value }
# CI：形如 `- name: 编码检查（*.ps1 必须有 BOM）`
$ciSteps = Select-String -Path $ciPath -Pattern '^\s*-\s+name:\s*(.+?)\s*$' |
    ForEach-Object { $_.Matches[0].Groups[1].Value }

$hookGates = @($hookSteps | ForEach-Object { Canonicalize $_ } | Where-Object { $_ } | Sort-Object -Unique)
$ciGates = @($ciSteps | ForEach-Object { Canonicalize $_ } | Where-Object { $_ } | Sort-Object -Unique)

Write-Output "钩子关口（$($hookGates.Count)）：$($hookGates -join '、')"
Write-Output "CI  关口（$($ciGates.Count)）：$($ciGates -join '、')"

# 允许单边存在的步骤：CI 独有的构建/打包，钩子独有的"通过"提示。
$ciOnly = @('Release 构建', '安装 bun', '缓存 Rust 产物')
$hookOnly = @()

$missingInCi = @($hookGates | Where-Object { $ciGates -notcontains $_ -and $hookOnly -notcontains $_ })
$missingInHook = @($ciGates | Where-Object { $hookGates -notcontains $_ -and $ciOnly -notcontains $_ })

$failed = 0
if ($missingInCi.Count -gt 0) {
    Write-Output "[FAIL] 钩子有、CI 没有：$($missingInCi -join '、')"
    $failed++
}
if ($missingInHook.Count -gt 0) {
    Write-Output "[FAIL] CI 有、钩子没有：$($missingInHook -join '、')"
    $failed++
}

if ($failed -gt 0) {
    Write-Output ''
    Write-Output '两边必须覆盖同一组关口，否则"本地全绿"就不代表 CI 会绿。'
    exit 1
}

# 另外确认一件事：钩子与 CI 必须用**同一条测试命令**，否则一个跑 workspace、
# 一个只跑单 crate，覆盖范围会悄悄不同。
$hookTest = ($hookSteps | Where-Object { $_ -match 'cargo test' }) -join ''
$ciTestLine = (Select-String -Path $ciPath -Pattern 'cargo test' | Select-Object -First 1).Line
if ($ciTestLine -and $hookTest -and ($ciTestLine -notmatch 'workspace') -and ($hookTest -match 'workspace')) {
    Write-Output "[FAIL] 钩子跑 --workspace，CI 没有：$($ciTestLine.Trim())"
    exit 1
}

Write-Output '零漂移：钩子与 CI 覆盖同一组关口。'
exit 0
