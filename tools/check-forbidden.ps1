# 检查"不可回退的约束"里那些**可以被机器查出来**的条目。
#
# 为什么需要它：`AGENTS.md` 的「不可回退的约束」里有一半以上是纯靠人记的
# （禁止引入 watcher / SQLite / 物理归档、presenter 不上锁、前端不注入 HTML）。
# 记性靠不住，而这几条一旦被破坏，代价是架构级的返工。
#
# 这里只查能被文本证据判定的部分，查不出的一律不假装覆盖
# （例如"禁止全文重序列化"要靠 format_closure 的往返测试，不在这里）。
$ErrorActionPreference = 'Stop'
Set-Location (Split-Path -Parent $PSScriptRoot)

$failures = @()

function Mask-Comments {
    param([string]$Text, [ref]$State)
    # 迷你词法器：把注释内容替换成空格，保留其余文本，跨行状态由 $State 携带。
    # 需要它是因为仓库里多处注释**正是在解释"为什么不用这个词"**——
    # 不屏蔽注释，检查就会误报，而误报的守卫会被关掉。
    $out = New-Object System.Text.StringBuilder
    $i = 0
    while ($i -lt $Text.Length) {
        if ($State.Value -eq 'html') {
            $close = $Text.IndexOf('-->', $i)
            if ($close -lt 0) { return (' ' * $Text.Length) }
            $out.Append(' ' * ($close + 3 - $i)) | Out-Null
            $i = $close + 3
            $State.Value = 'code'
            continue
        }
        if ($State.Value -eq 'block') {
            $close = $Text.IndexOf('*/', $i)
            if ($close -lt 0) { return (' ' * $Text.Length) }
            $out.Append(' ' * ($close + 2 - $i)) | Out-Null
            $i = $close + 2
            $State.Value = 'code'
            continue
        }
        $rest = $Text.Substring($i)
        if ($rest.StartsWith('<!--')) { $State.Value = 'html'; $out.Append('    ') | Out-Null; $i += 4; continue }
        if ($rest.StartsWith('/*')) { $State.Value = 'block'; $out.Append('  ') | Out-Null; $i += 2; continue }
        if ($rest.StartsWith('//')) { $out.Append(' ' * $rest.Length) | Out-Null; break }
        $out.Append($Text[$i]) | Out-Null
        $i++
    }
    return $out.ToString()
}

function Check-Forbidden {
    param(
        [string]$Label,
        [string]$Pattern,
        [string[]]$Include,
        [string]$Reason,
        [switch]$SkipComments
    )
    $hits = @()
    foreach ($glob in $Include) {
        $files = Get-ChildItem -Path . -Recurse -File -Filter $glob -ErrorAction SilentlyContinue |
            Where-Object { $_.FullName -notmatch '\\target\\' -and $_.FullName -notmatch '\\node_modules\\' }
        foreach ($file in $files) {
            $state = 'code'
            $scanState = [ref]$state
            $lineNo = 0
            foreach ($line in [System.IO.File]::ReadAllLines($file.FullName)) {
                $lineNo++
                $scan = if ($SkipComments) { Mask-Comments -Text $line -State $scanState } else { $line }
                if ($scan -match $Pattern) {
                    $hits += ("{0}:{1}: {2}" -f (Resolve-Path -Relative $file.FullName), $lineNo, $line.Trim())
                }
            }
        }
    }
    if ($hits.Count -gt 0) {
        Write-Output "  [禁止] $Label —— $Reason"
        $hits | Select-Object -First 6 | ForEach-Object { Write-Output "         $_" }
        if ($hits.Count -gt 6) { Write-Output ("         ……另有 " + ($hits.Count - 6) + " 处") }
        $script:failures += $Label
    } else {
        Write-Output "  [通过] $Label"
    }
}

Write-Output "== 禁止引入的依赖（AGENTS.md「不可回退的约束」）=="
Check-Forbidden -Label "文件 watcher（notify 等）" `
    -Pattern '^\s*(notify|notify-debouncer-mini|tauri-plugin-fs-watch)\s*=' `
    -Include @('Cargo.toml') `
    -Reason "外部变化只靠写入前读盘比对发现，watcher 会引入第二套时序"

Check-Forbidden -Label "SQLite 及其驱动" `
    -Pattern '^\s*(rusqlite|sqlx|diesel|libsqlite3-sys)\s*=' `
    -Include @('Cargo.toml') `
    -Reason "inbox.md 是唯一真源，索引会造出第二份事实"

Write-Output "== 禁止的代码形态 =="
Check-Forbidden -Label "物理归档目录 archive.md" `
    -Pattern '"archive\.md"|archive\.md' `
    -Include @('*.rs') `
    -Reason "归档只是 marker 上的墓碑，不得转移到第二个文件"

Check-Forbidden -Label "给 presenter 上锁（破坏单队列单消费者）" `
    -Pattern 'Mutex<\s*Presenter\s*>|RwLock<\s*Presenter\s*>' `
    -Include @('*.rs') `
    -Reason "presenter 由消费者独占；上锁意味着出现了第二个写者"

Check-Forbidden -Label "前端 HTML 注入（{@html）" `
    -Pattern '\{@html' `
    -Include @('*.svelte', '*.js') `
    -Reason "inbox.md 是用户可编辑的文件，它的内容不能当 HTML 执行" `
    -SkipComments

Write-Output ""
if ($failures.Count -gt 0) {
    Write-Output ("违反 " + $failures.Count + " 条不可回退的约束：" + ($failures -join '、'))
    Write-Output "这些条目在 AGENTS.md 里有明确理由，改动前先读设计文档对应小节。"
    exit 1
}
Write-Output "全部通过：没有触碰不可回退的约束。"
exit 0
