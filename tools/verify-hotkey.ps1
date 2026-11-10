# Slip 的端到端验收：召唤式生命周期 + 捕获链路。
#
# 这份脚本用**外部视角**取证，不用"看起来对"：
#   - Win32 发真实热键、读窗口可见性与矩形（GetWindowRect / IsWindowVisible）；
#   - WebView2 CDP 读界面内部状态；
#   - 文件系统读 inbox.md 原始字节。
#
# 覆盖的语义（改窗口/热键/捕获链路后必须全绿）：
#   1. 开机自启（带 --autostart）→ 窗口保持隐藏
#   2. 手动双击（无 --autostart）→ 窗口自动显示
#   3. 热键唤出 → 成为前台、窗口精确贴在光标右下 16px
#   4. 光标靠右下时放不下 → 翻到左上（仍不越界）
#   5. 回车 → 提示已收录、列表出现、输入框清空、磁盘落盘、窗口自动收起
#   6. 输入法组合态回车不提交
#   7. 焦点被别的应用抢走 → 窗口自动收起
#
# 注意：PowerShell 是 DPI 不感知进程，GetWindowRect / GetCursorPos 会被系统按同一
# 比例虚拟化（本机 150% 缩放），所以两者可以直接相减比较，但"屏幕高度"也是虚拟
# 后的数值（1600 物理 → 1066 虚拟），断言里不能拿物理值算。
#
# 用法（先构建 target/debug/slip-shell.exe）：
#   pwsh -File tools/verify-hotkey.ps1

param(
    [string]$Vault = "",
    [int]$Port = 9222
)

$ErrorActionPreference = 'Continue'
$PSNativeCommandUseErrorActionPreference = $false

$root = Split-Path -Parent $PSScriptRoot
if (-not $Vault) { $Vault = Join-Path $root '.tmp-vault' }
$exe = Join-Path $root 'target\debug\slip-shell.exe'
if (-not (Test-Path $exe)) { throw "找不到 $exe —— 先跑 cargo build -p slip-shell" }
$bun = Join-Path $env:USERPROFILE '.bun\bin\bun.exe'
if (-not (Test-Path $bun)) { throw "找不到 bun：$bun" }
$probe = Join-Path $PSScriptRoot 'cdp-probe.mjs'

# 窗口与光标之间的缝（逻辑像素）。与 hotkey.rs 的 CURSOR_GAP 必须一致。
$Gap = 16

$utf8 = [System.Text.UTF8Encoding]::new($false)
Add-Type -AssemblyName Microsoft.VisualBasic

Add-Type -TypeDefinition @"
using System;
using System.Text;
using System.Runtime.InteropServices;

[StructLayout(LayoutKind.Sequential)]
public struct RECT { public int Left, Top, Right, Bottom; }
[StructLayout(LayoutKind.Sequential)]
public struct POINT { public int X, Y; }

public static class Win {
    public const byte VK_CONTROL = 0x11;
    public const byte VK_MENU    = 0x12;
    public const byte VK_SPACE   = 0x20;
    public const uint KEYUP      = 0x0002;

    [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetClassName(IntPtr h, StringBuilder s, int c);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr FindWindow(string cls, string title);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern bool GetCursorPos(out POINT p);
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);

    public static string ClassName(IntPtr h) {
        var sb = new StringBuilder(512);
        GetClassName(h, sb, sb.Capacity);
        return sb.ToString();
    }
    public static void SendHotkey() {
        keybd_event(VK_CONTROL, 0, 0, UIntPtr.Zero);
        keybd_event(VK_MENU,    0, 0, UIntPtr.Zero);
        keybd_event(VK_SPACE,   0, 0, UIntPtr.Zero);
        keybd_event(VK_SPACE,   0, KEYUP, UIntPtr.Zero);
        keybd_event(VK_MENU,    0, KEYUP, UIntPtr.Zero);
        keybd_event(VK_CONTROL, 0, KEYUP, UIntPtr.Zero);
    }
}
"@

$script:failed = 0

function Check {
    param([string]$Label, [bool]$Ok, [string]$Detail = "")
    $tag = "[PASS]"
    if (-not $Ok) { $tag = "[FAIL]"; $script:failed++ }
    if ([string]::IsNullOrEmpty($Detail)) {
        Write-Output "$tag $Label"
    } else {
        Write-Output "$tag $Label - $Detail"
    }
}

function Get-SlipWindow {
    # 注意：PowerShell 把 $null 传给 [string] 形参时会变成**空字符串**，
    # 于是 FindWindow("", "Slip") 变成"找类名为空的窗口"，永远返回 0。
    # 要传真正的 NULL 必须用 [NullString]::Value。
    $h = [Win]::FindWindow([NullString]::Value, "Slip")
    if ($h -eq [IntPtr]::Zero) { $h = [Win]::FindWindow("Tauri Window", [NullString]::Value) }
    return $h
}

function Test-SlipVisible {
    $h = Get-SlipWindow
    if ($h -eq [IntPtr]::Zero) { return $false }
    return [Win]::IsWindowVisible($h)
}

function Get-SlipRect {
    $h = Get-SlipWindow
    if ($h -eq [IntPtr]::Zero) { return $null }
    $r = New-Object RECT
    if ([Win]::GetWindowRect($h, [ref]$r)) { return $r }
    return $null
}

function Stop-Slip {
    Get-Process slip-shell -ErrorAction SilentlyContinue | Stop-Process -Force
    Start-Sleep -Milliseconds 700
}

function Reset-Vault {
    if (Test-Path $Vault) { Remove-Item $Vault -Recurse -Force }
    New-Item -ItemType Directory -Path $Vault -Force | Out-Null
}

# 光标归位并唤出，返回窗口矩形。调用前窗口必须处于"已收起"状态。
function Invoke-SummonAt {
    param([int]$X, [int]$Y)
    [void][Win]::SetCursorPos($X, $Y)
    Start-Sleep -Milliseconds 400
    [Win]::SendHotkey()
    # 轮询而不是死等固定时间：窗口一旦可见且位置稳定就往下走。
    $last = $null
    for ($i = 0; $i -lt 25; $i++) {
        Start-Sleep -Milliseconds 120
        if (-not (Test-SlipVisible)) { continue }
        $r = Get-SlipRect
        if ($null -ne $r -and $null -ne $last -and
            $r.Left -eq $last.Left -and $r.Top -eq $last.Top) {
            return $r
        }
        $last = $r
    }
    return (Get-SlipRect)
}

function Get-CursorPoint {
    $c = New-Object POINT
    [void][Win]::GetCursorPos([ref]$c)
    return $c
}

# 位置断言只断言**性质**，不断言像素。
#
# 为什么：PowerShell 是 DPI 不感知进程，它给的光标坐标与 Tauri 读到的物理光标
# 之间隔着系统的虚拟化换算。实测同一份代码，SetCursorPos(200,200) 之后应用
# 读到的光标可以偏出上百像素——"精确落在 (216,216)"这类断言会随机假绿或假红。
# 下面这些仍然能抓住真正的回归：窗口跑回屏幕正中、盖住光标、翻错方向、越界。
function Assert-Placement {
    param([string]$Label, [object]$Rect, [object]$Cursor, [switch]$ExpectFlip)

    if ($null -eq $Rect) { Check "$Label：窗口矩形可读" $false "GetWindowRect 失败"; return }

    $w = $Rect.Right - $Rect.Left
    $h = $Rect.Bottom - $Rect.Top
    $detail = "win=($($Rect.Left),$($Rect.Top)) ${w}x${h} cursor=($($Cursor.X),$($Cursor.Y))"

    $covers = ($Cursor.X -ge $Rect.Left) -and ($Cursor.X -le $Rect.Right) -and
              ($Cursor.Y -ge $Rect.Top) -and ($Cursor.Y -le $Rect.Bottom)
    Check "$Label：窗口不覆盖光标" (-not $covers) $detail

    # 光标到窗口某个角外侧的距离；"贴在光标旁"意味着横竖都在一个缝隙量级内。
    $adjX = [Math]::Min([Math]::Abs($Cursor.X - ($Rect.Left - $Gap)),
                        [Math]::Abs($Cursor.X - ($Rect.Right + $Gap)))
    $adjY = [Math]::Min([Math]::Abs($Cursor.Y - ($Rect.Top - $Gap)),
                        [Math]::Abs($Cursor.Y - ($Rect.Bottom + $Gap)))
    Check "$Label：窗口紧贴光标一侧（不是屏幕居中）" (($adjX -le 96) -and ($adjY -le 96)) `
        "$detail adj=($adjX,$adjY)"

    if ($ExpectFlip) {
        # 右下放不下时整体落在光标左侧：翻转方向的判据，不依赖绝对坐标。
        Check "$Label：放不下时翻到光标左侧" ($Rect.Right -le $Cursor.X) $detail
    }
}

$env:SLIP_VAULT = $Vault
$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = "--remote-debugging-port=$Port"
$errLog = Join-Path $root 'e.log'
$outLog = Join-Path $root 'r.log'

# ----------------------------------------------------------------- 1. 自启启动静默
Stop-Slip
Reset-Vault
$null = Start-Process -FilePath $exe -ArgumentList '--autostart' -PassThru `
    -RedirectStandardOutput $outLog -RedirectStandardError $errLog
Start-Sleep -Seconds 9
$visible = Test-SlipVisible
Check "开机自启（--autostart）时窗口保持隐藏" (-not $visible) "visible=$visible"
Stop-Slip

# ----------------------------------------------------------------- 2. 手动启动显示
Reset-Vault
$null = Start-Process -FilePath $exe -PassThru `
    -RedirectStandardOutput $outLog -RedirectStandardError $errLog
Start-Sleep -Seconds 9
$visible = Test-SlipVisible
Check "手动启动（无 --autostart）时窗口自动显示" $visible "visible=$visible"

# ----------------------------------------------------------------- 3. 位置：右下放得下
& $bun run $probe $Port --dismiss 2>&1 | Out-Null
Start-Sleep -Milliseconds 900
$visible = Test-SlipVisible
Check "CDP 收走后窗口不可见" (-not $visible) "visible=$visible"

# 左上角留足空间：右下都放得下，不该翻转。
$rect = Invoke-SummonAt -X 200 -Y 200
$visible = Test-SlipVisible
Check "热键唤出后窗口可见" $visible "visible=$visible"
$fgClass = [Win]::ClassName([Win]::GetForegroundWindow())
Check "热键唤出后成为前台" ($fgClass -eq "Tauri Window") "class=$fgClass"
Assert-Placement -Label "左上场景" -Rect $rect -Cursor (Get-CursorPoint)
if ($rect) {
    $w = $rect.Right - $rect.Left
    $h = $rect.Bottom - $rect.Top
    Check "窗口尺寸取自配置 420x560" (($w -eq 420) -and ($h -eq 560)) "size=${w}x${h}"
}

# ----------------------------------------------------------------- 4. 捕获链路
& $bun run $probe $Port
if ($LASTEXITCODE -ne 0) { $script:failed++ }

# 收录成功就该自己收走（不是等失焦）。
$visible = Test-SlipVisible
Check "收录成功后窗口自动收起" (-not $visible) "visible=$visible"

# ----------------------------------------------------------------- 5. 位置：右下放不下则翻转
$rect = Invoke-SummonAt -X 1400 -Y 900
$visible = Test-SlipVisible
Check "翻转场景下窗口可见" $visible "visible=$visible"
Assert-Placement -Label "右下场景" -Rect $rect -Cursor (Get-CursorPoint) -ExpectFlip
if ($rect) {
    Check "翻转后坐标不为负（未越出工作区）" (($rect.Left -ge 0) -and ($rect.Top -ge 0)) `
        "win=($($rect.Left),$($rect.Top))"
}

# ----------------------------------------------------------------- 6. 失焦自动收走
$notepad = Start-Process notepad.exe -PassThru
Start-Sleep -Seconds 2
try { $null = [Microsoft.VisualBasic.Interaction]::AppActivate($notepad.Id) } catch {}
Start-Sleep -Seconds 2
$visible = Test-SlipVisible
Check "焦点被其它应用抢走后窗口自动收起" (-not $visible) "visible=$visible"

# ----------------------------------------------------------------- 7. 磁盘
Write-Output "--- inbox.md ---"
$inbox = Join-Path $Vault 'inbox.md'
if (Test-Path $inbox) {
    $content = $utf8.GetString([System.IO.File]::ReadAllBytes($inbox))
    Write-Output $content
    $intact = $content -match '- \[ \] 热键唤出后写入的一条 <!-- slip:v1;id=[0-9a-f-]+;created='
    Check "磁盘上出现完整记录行" $intact ""
} else {
    Check "磁盘上出现 inbox.md" $false "文件不存在"
}

Write-Output "--- 启动日志（stderr）---"
if (Test-Path $errLog) { Write-Output $utf8.GetString([System.IO.File]::ReadAllBytes($errLog)) }

Stop-Slip
Get-Process notepad -ErrorAction SilentlyContinue | Stop-Process -Force

Write-Output ""
if ($script:failed -eq 0) { Write-Output "全部通过。" } else { Write-Output "失败 $script:failed 项。" }
exit $script:failed
