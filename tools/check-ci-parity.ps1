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
param(
    # 自检：验证「绕过门禁的写法」识别器本身没写死。
    [switch]$SelfTest
)

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
    @{ Key = 'BOM'; Name = '编码（BOM）' }
    @{ Key = '语法||syntax'; Name = 'PowerShell 语法' }
    @{ Key = '死代码||dead-constants'; Name = '常量与镜像' }
    @{ Key = '文档与代码同步||doc-sync'; Name = '文档同步' }
    @{ Key = '门禁对等性||ci-parity'; Name = '门禁对等性' }
    @{ Key = '不可回退||forbidden'; Name = '不可回退约束' }
    @{ Key = '纯逻辑||ui-logic'; Name = '前端纯逻辑' }
    @{ Key = '产物体积||bundle'; Name = '前端产物体积' }
    # 注意顺序：`前端格式` 必须排在通用的 `格式检查` 之前，否则 CI 的
    # 「前端格式检查」会被先命中的 `格式检查` 抢走、误判成 Rust 格式。
    @{ Key = 'prettier||前端格式'; Name = '前端格式' }
    @{ Key = '前端产物||构建前端'; Name = '前端构建' }
    @{ Key = 'cargo fmt||格式检查'; Name = 'Rust 格式' }
    @{ Key = 'clippy'; Name = 'Clippy' }
    @{ Key = 'cargo test||测试'; Name = '测试' }
)

function Canonicalize([string]$Text) {
    foreach ($gate in $gates) {
        foreach ($kw in $gate.Key.Split('||')) {
            if ($Text -match [regex]::Escape($kw)) { return $gate.Name }
        }
    }
    return $null
}

# 允许**单边存在**的步骤，按原始步骤名精确匹配。
#
# 旧实现是 `Where-Object { $_ }`：归一到 $null 的步骤被静默丢掉，于是
# 「常量 / 文档同步 / 门禁对等性」三步从来没参与过比对——它们两边都跑了，
# 但即使只在一侧跑，脚本也不会出声。未识别的步骤现在默认 [FAIL]，
# 想放行必须在这里写明白，并且说明理由。
$hookOnly = @(
    '通过',                                  # 钩子的收尾提示，不是关口
    '前端产物缺失或陈旧，先构建',            # 归一到「前端构建」，只在需要时跑
    'PowerShell 语法检查（跳过：环境里没有 pwsh）'  # 无 pwsh 时的降级分支
)
$ciOnly = @(
    'Release 构建',                          # debug 能过不代表 release 能过，只在 CI 跑
    '安装 bun',
    '缓存 Rust 产物',
    '安装 Rust 工具链',
    '安装 Tauri Linux 构建依赖',
    '构建前端产物',                          # CI 用独立 step 名，「构建前端产物」归一到前端构建
    '逻辑层测试（domain / store / app）',    # Linux 侧的第二组证据，Windows 钩子跑不了
    '逻辑层 Clippy',
    'Linux 外壳 Clippy',
    'Linux 外壳构建',
    'Linux 外壳单测',
    '变异审计（核心不变量）'          # 只在 Linux CI 跑，Windows 钩子不重复跑
)

# 钩子：形如 `step "编码检查（*.ps1 必须有 BOM）"`
$hookSteps = @(Select-String -Path $hookPath -Pattern '^\s*step\s+"([^"]+)"' |
    ForEach-Object { $_.Matches[0].Groups[1].Value })
# CI：形如 `- name: 编码检查（*.ps1 必须有 BOM）`
$ciSteps = @(Select-String -Path $ciPath -Pattern '^\s*-\s+name:\s*(.+?)\s*$' |
    ForEach-Object { $_.Matches[0].Groups[1].Value })

$failed = 0

# **必须存在的关口全集**：只比两侧差集不够——两边同时删掉同一个关口时，
# 差集为空、脚本照样报「零漂移」。这份清单是关口的底线。
$requiredGates = @(
    '编码（BOM）', 'PowerShell 语法', '常量与镜像', '文档同步', '门禁对等性',
    '不可回退约束', '前端纯逻辑', '前端产物体积', '前端格式', '前端构建',
    'Rust 格式', 'Clippy', '测试'
)

# 这些写法会让一个「看起来存在」的步骤实际不拦任何东西。
function Test-ForbiddenCommand([string]$Text) {
    return [regex]::IsMatch(
        $Text,
        '\|\|\s*true\b|continue-on-error\s*:\s*true|if\s*:\s*false\b'
    )
}
foreach ($pair in @(
        @{ Name = '钩子'; Path = $hookPath }
        @{ Name = 'CI'; Path = $ciPath }
    )) {
    $hits = Select-String -Path $pair.Path -Pattern '\|\|\s*true\b|continue-on-error\s*:\s*true|if\s*:\s*false\b'
    foreach ($hit in $hits) {
        Write-Output "[FAIL] $($pair.Name) 第 $($hit.LineNumber) 行含绕过门禁的写法：$($hit.Line.Trim())"
        $failed++
    }
}

# 第一步：每一步都必须能被归一，否则它**根本没参与比对**。
foreach ($step in $hookSteps) {
    if ($hookOnly -contains $step) { continue }
    if ($null -eq (Canonicalize $step)) {
        Write-Output "[FAIL] 钩子步骤无法归一，未参与比对：$step"
        Write-Output '       要么把关键词加进 $gates，要么写进 $hookOnly 并说明理由。'
        $failed++
    }
}
foreach ($step in $ciSteps) {
    if ($ciOnly -contains $step) { continue }
    if ($null -eq (Canonicalize $step)) {
        Write-Output "[FAIL] CI 步骤无法归一，未参与比对：$step"
        Write-Output '       要么把关键词加进 $gates，要么写进 $ciOnly 并说明理由。'
        $failed++
    }
}

$hookGates = @($hookSteps | ForEach-Object { Canonicalize $_ } | Where-Object { $_ } | Sort-Object -Unique)
$ciGates = @($ciSteps | ForEach-Object { Canonicalize $_ } | Where-Object { $_ } | Sort-Object -Unique)

Write-Output "钩子关口（$($hookGates.Count)）：$($hookGates -join '、')"
Write-Output "CI  关口（$($ciGates.Count)）：$($ciGates -join '、')"

foreach ($gate in $requiredGates) {
    if ($hookGates -notcontains $gate) {
        Write-Output "[FAIL] 必需关口在钩子里缺失：$gate"
        $failed++
    }
    if ($ciGates -notcontains $gate) {
        Write-Output "[FAIL] 必需关口在 CI 里缺失：$gate"
        $failed++
    }
}

$missingInCi = @($hookGates | Where-Object { $ciGates -notcontains $_ })
$missingInHook = @($ciGates | Where-Object { $hookGates -notcontains $_ })

if ($missingInCi.Count -gt 0) {
    Write-Output "[FAIL] 钩子有、CI 没有：$($missingInCi -join '、')"
    $failed++
}
if ($missingInHook.Count -gt 0) {
    Write-Output "[FAIL] CI 有、钩子没有：$($missingInHook -join '、')"
    $failed++
}

# 第二步：同一关口两侧的**命令片段**必须等价。
#
# 只比对「标签存在」是不够的：钩子写 `cargo clippy --all-targets -- -D warnings`、
# CI 写成 `cargo clippy`，标签一样、覆盖范围差一大截。下面按关口逐条断言片段。
function Get-HookBlock([string]$StepName) {
    # 从 step 行开始，取到下一个 step 之前：这样能覆盖写在同一段里的命令。
    $lines = Get-Content $hookPath
    $start = -1
    for ($i = 0; $i -lt $lines.Count; $i++) {
        if ($lines[$i] -match ('^\s*step\s+"' + [regex]::Escape($StepName) + '"')) { $start = $i; break }
    }
    if ($start -lt 0) { return '' }
    $end = $lines.Count
    for ($i = $start + 1; $i -lt $lines.Count; $i++) {
        if ($lines[$i] -match '^\s*step\s+"') { $end = $i; break }
    }
    # **从步骤行的下一行开始**：步骤标签里常常就写着脚本名
    # （`step "前端产物体积（tools/check-bundle-size.ps1）"`），
    # 把标签算作命令证据会让「命令换成 true」仍然 PASS——实测过。
    # 同时剥掉注释行：注释里也可以写 `-SelfTest`，同样不是证据。
    if ($start + 1 -ge $end) { return '' }
    $block = $lines[($start + 1)..($end - 1)] | Where-Object { $_ -notmatch '^\s*#' }
    return ($block -join "`n")
}

function Get-CiBlock([string]$StepName) {
    $lines = Get-Content $ciPath
    $start = -1
    for ($i = 0; $i -lt $lines.Count; $i++) {
        if ($lines[$i] -match ('^\s*-\s+name:\s*' + [regex]::Escape($StepName) + '\s*$')) { $start = $i; break }
    }
    if ($start -lt 0) { return '' }
    $end = $lines.Count
    for ($i = $start + 1; $i -lt $lines.Count; $i++) {
        if ($lines[$i] -match '^\s*-\s+name:') { $end = $i; break }
    }
    # 同上：步骤名与注释都不算命令证据。
    if ($start + 1 -ge $end) { return '' }
    $block = $lines[($start + 1)..($end - 1)] | Where-Object { $_ -notmatch '^\s*#' }
    return ($block -join "`n")
}

# 每个条目：钩子步骤名、CI 步骤名、两侧都必须出现的片段。
#
# 覆盖全部 13 个关口（BOM 靠标签比对，不单独断言片段）。只查四个关口时，
# 把 bundle-size / prettier 的命令换成 `true`、给 clippy 加 `|| true`，
# 脚本仍会报「零漂移」——而它存在的意义就是发现这种事。
$checks = @(
    @{ Hook = 'PowerShell 语法检查'; Ci = 'PowerShell 语法检查'
       Must = @('check-ps1-syntax.ps1') }
    @{ Hook = '常量：死代码与前后端镜像（tools/check-dead-constants.ps1）'
       Ci = '常量：死代码与前后端镜像'
       Must = @('check-dead-constants.ps1') }
    @{ Hook = '设计文档与代码同步（tools/check-doc-sync.ps1）'
       Ci = '设计文档与代码同步'
       Must = @('check-doc-sync.ps1') }
    @{ Hook = '钩子与 CI 的门禁对等性（tools/check-ci-parity.ps1）'
       Ci = '门禁对等性（钩子与 CI 零漂移）'
       Must = @('check-ci-parity.ps1') }
    @{ Hook = '不可回退的约束（禁止引入清单）'; Ci = '不可回退的约束（禁止引入清单）'
       Must = @('check-forbidden.ps1', '-SelfTest') }
    @{ Hook = '前端纯逻辑检查（tools/check-ui-logic.mjs）'; Ci = '前端纯逻辑检查'
       Must = @('check-ui-logic.mjs') }
    @{ Hook = '前端产物体积（tools/check-bundle-size.ps1）'; Ci = '前端产物体积门禁'
       Must = @('check-bundle-size.ps1') }
    @{ Hook = 'prettier --check（ui）'; Ci = '前端格式检查'
       Must = @('format:check') }
    @{ Hook = '前端产物缺失或陈旧，先构建'; Ci = '构建前端产物'
       Must = @('--frozen-lockfile', 'run build') }
    @{ Hook = 'cargo clippy --workspace --all-targets -D warnings'; Ci = 'Clippy（警告即错误）'
       Must = @('--all-targets', '-D warnings') }
    @{ Hook = 'cargo test --workspace'; Ci = '测试'
       Must = @('--workspace') }
    @{ Hook = 'cargo fmt --all --check'; Ci = '格式检查'
       Must = @('fmt --all', '--check') }
)

foreach ($check in $checks) {
    $hookBlock = Get-HookBlock $check.Hook
    $ciBlock = Get-CiBlock $check.Ci
    if (-not $hookBlock) {
        Write-Output "[FAIL] 找不到钩子步骤：$($check.Hook)"
        $failed++
        continue
    }
    if (-not $ciBlock) {
        Write-Output "[FAIL] 找不到 CI 步骤：$($check.Ci)"
        $failed++
        continue
    }
    foreach ($fragment in $check.Must) {
        if ($hookBlock -notmatch [regex]::Escape($fragment)) {
            Write-Output "[FAIL] 钩子步骤「$($check.Hook)」缺少片段：$fragment"
            $failed++
        }
        if ($ciBlock -notmatch [regex]::Escape($fragment)) {
            Write-Output "[FAIL] CI 步骤「$($check.Ci)」缺少片段：$fragment"
            $failed++
        }
    }
}

if ($SelfTest) {
    $bad = 0
    $cases = @(
        @{ Name = '|| true'; Text = 'cargo clippy --all-targets -- -D warnings || true'; Want = $true }
        @{ Name = 'continue-on-error'; Text = 'continue-on-error: true'; Want = $true }
        @{ Name = 'if false'; Text = 'if: false'; Want = $true }
        @{ Name = '正常 clippy'; Text = 'cargo clippy --all-targets -- -D warnings'; Want = $false }
        @{ Name = '正常测试'; Text = 'cargo test --workspace --locked'; Want = $false }
    )
    foreach ($case in $cases) {
        $got = Test-ForbiddenCommand $case.Text
        if ($got -eq $case.Want) { Write-Output "  [PASS] $($case.Name)" }
        else { Write-Output "  [FAIL] $($case.Name)：识别结果 $got，期望 $($case.Want)"; $bad++ }
    }
    if ($bad -gt 0) { exit 1 }
    Write-Output '自检通过：绕过门禁的写法能被识别。'
    exit 0
}

if ($failed -gt 0) {
    Write-Output ''
    Write-Output '两边必须覆盖同一组关口与同一条命令，否则"本地全绿"就不代表 CI 会绿。'
    exit 1
}

Write-Output '零漂移：钩子与 CI 覆盖同一组关口。'
exit 0
