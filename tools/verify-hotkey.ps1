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
Add-Type -AssemblyName System.Windows.Forms

# 场景坐标一律由**当前**主屏尺寸推导，不写死。
#
# 教训：原先翻转场景把光标钉在 (1400,900)，在"2560x1600 + 3840x2160 双屏"下
# 右下确实放不下；显示器改成单块 2560x1440 之后，1400+16+420=1836 ≤ 2560，
# 水平方向本来就放得下，于是"必须翻到左侧"的断言变成假红。
# 断言依赖的是"放不下"这个前提，坐标就必须跟着屏幕算。
$primaryBounds = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
$ScreenW = $primaryBounds.Width
$ScreenH = $primaryBounds.Height
# 左上场景：留足空白，保证右下放得下（不触发翻转）。
$NearX = [Math]::Max(40, [int]($ScreenW * 0.1))
$NearY = [Math]::Max(40, [int]($ScreenH * 0.1))
# 翻转场景：贴到右下角，保证两个轴都放不下。
$FarX = [Math]::Max(40, $ScreenW - 60)
$FarY = [Math]::Max(40, $ScreenH - 60)

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
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    public delegate bool EnumProc(IntPtr h, IntPtr p);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr p);

    /// 该进程的全部顶层窗口。
    public static System.Collections.Generic.List<IntPtr> TopWindowsOf(uint want) {
        var list = new System.Collections.Generic.List<IntPtr>();
        EnumWindows((h, p) => {
            uint pid;
            GetWindowThreadProcessId(h, out pid);
            if (pid == want) list.Add(h);
            return true;
        }, IntPtr.Zero);
        return list;
    }

    public static uint PidOf(IntPtr h) {
        uint pid;
        GetWindowThreadProcessId(h, out pid);
        return pid;
    }

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
$script:slipPid = 0

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

# 按**进程 ID** 找窗口，不按标题/类名。
#
# 教训：类名 "Tauri Window" 一点都不独特——Clash Verge 之类的 Tauri 应用用的是同一个类名。
# 早先的实现用 FindWindow("Tauri Window", NULL) 兜底，结果在同时装着别的 Tauri 应用
# 的机器上抓到了别人的窗口，可见性与位置断言全部读错对象，报出一堆假失败。
function Get-SlipWindow {
    if ($script:slipPid -le 0) { return [IntPtr]::Zero }
    foreach ($h in [Win]::TopWindowsOf([uint32]$script:slipPid)) {
        if ([Win]::ClassName($h) -eq "Tauri Window") { return $h }
    }
    return [IntPtr]::Zero
}

function Test-SlipVisible {
    $h = Get-SlipWindow
    if ($h -eq [IntPtr]::Zero) { return $false }
    return [Win]::IsWindowVisible($h)
}

# 在给定时间窗内**是否出现过**可见状态。
#
# 手动启动的意图是"让用户看见窗口"，不是"保证它一直挂着"：一旦别的应用抢走
# 前台，失焦收走是设计行为。用"9 秒后仍可见"来断言，会在有其它程序反复抢
# 焦点的机器上随机失败——那是断言错了，不是产品错了。
function Wait-OnceVisible {
    param([int]$TimeoutMs = 6000)
    $deadline = [DateTime]::Now.AddMilliseconds($TimeoutMs)
    while ([DateTime]::Now -lt $deadline) {
        if (Test-SlipVisible) { return $true }
        Start-Sleep -Milliseconds 100
    }
    return $false
}

function Get-SlipRect {
    $h = Get-SlipWindow
    if ($h -eq [IntPtr]::Zero) { return $null }
    $r = New-Object RECT
    if ([Win]::GetWindowRect($h, [ref]$r)) { return $r }
    return $null
}

# 前台是否真的是我们：比进程 ID，不比窗口类名（理由同上）。
function Test-SlipForeground {
    $fg = [Win]::GetForegroundWindow()
    if ($fg -eq [IntPtr]::Zero) { return $false }
    return ([Win]::PidOf($fg) -eq [uint32]$script:slipPid)
}

function Start-Slip {
    param([string[]]$Arguments = @())
    $p = Start-Process -FilePath $exe -ArgumentList $Arguments -PassThru `
        -RedirectStandardOutput $outLog -RedirectStandardError $errLog
    $script:slipPid = $p.Id
    return $p
}

function Stop-Slip {
    Get-Process slip-shell -ErrorAction SilentlyContinue | Stop-Process -Force
    $script:slipPid = 0
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
    # 在发热键**之前**采样光标：应用读到的就是这一刻的位置。
    # 若等唤出之后再读，鼠标一动这段断言就会漂掉（实测能漂上百像素）。
    $script:summonCursor = Get-CursorPoint
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
        # 右下都放不下时，窗口整体落在光标的左上方。
        # 判据只依赖"放不下"这个前提，所以召唤坐标必须由当前屏幕尺寸推导——
        # 写死坐标会在显示器数量/分辨率变化后变成假红。
        Check "$Label：放不下时翻到光标左侧" ($Rect.Right -le $Cursor.X) $detail
        Check "$Label：放不下时翻到光标上方" ($Rect.Bottom -le $Cursor.Y) $detail
    }
}

$env:SLIP_VAULT = $Vault
$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = "--remote-debugging-port=$Port"
$errLog = Join-Path $root 'e.log'
$outLog = Join-Path $root 'r.log'

# ----------------------------------------------------------------- 1. 自启启动静默
Stop-Slip
Reset-Vault
$null = Start-Slip -Arguments @('--autostart')
Start-Sleep -Seconds 9
$visible = Test-SlipVisible
Check "开机自启（--autostart）时窗口保持隐藏" (-not $visible) "visible=$visible"
Stop-Slip

# ----------------------------------------------------------------- 2. 手动启动显示
Reset-Vault
$null = Start-Slip
$appeared = Wait-OnceVisible
Check "手动启动（无 --autostart）时会主动显示窗口" $appeared "appeared=$appeared"
# 启动展示**不挂**失焦收走：别的程序抢前台不该让窗口消失。
# 本机 Clash Verge 约每 2.7~4.5 秒抢一次前台，这条断言正是挡住
# "启动后一个窗口都没有"那个回归的。
Start-Sleep -Seconds 6
$still = Test-SlipVisible
Check "启动展示不会被其它应用抢焦点而收走" $still "visible=$still"

# ----------------------------------------------------------------- 3. 位置：右下放得下
& $bun run $probe $Port --dismiss 2>&1 | Out-Null
Start-Sleep -Milliseconds 900
$visible = Test-SlipVisible
Check "CDP 收走后窗口不可见" (-not $visible) "visible=$visible"

# 左上角留足空间：右下都放得下，不该翻转。
$rect = Invoke-SummonAt -X $NearX -Y $NearY
$visible = Test-SlipVisible
Check "热键唤出后窗口可见" $visible "visible=$visible"
Check "热键唤出后成为前台" (Test-SlipForeground) "pid=$script:slipPid fgPid=$([Win]::PidOf([Win]::GetForegroundWindow()))"
Assert-Placement -Label "左上场景" -Rect $rect -Cursor $script:summonCursor
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
$rect = Invoke-SummonAt -X $FarX -Y $FarY
$visible = Test-SlipVisible
Check "翻转场景下窗口可见" $visible "visible=$visible"
Assert-Placement -Label "右下场景" -Rect $rect -Cursor $script:summonCursor -ExpectFlip
if ($rect) {
    Check "翻转后坐标不为负（未越出工作区）" (($rect.Left -ge 0) -and ($rect.Top -ge 0)) `
        "win=($($rect.Left),$($rect.Top))"
}

# ----------------------------------------------------------------- 6. 位置：负坐标副屏
#
# 这条路径原先完全没有覆盖：主屏坐标都是正的，而副屏可能整体落在负坐标区
# （本机 DISPLAY5 在 X=-454, Y=-1440）。摆放用的是"光标所在显示器的工作区"，
# 一旦把边界当成 0 来夹紧，窗口就会跑回主屏或屏幕外。
& $bun run $probe $Port --dismiss 2>&1 | Out-Null
Start-Sleep -Milliseconds 700

$other = [System.Windows.Forms.Screen]::AllScreens |
    Where-Object { -not $_.Primary } | Select-Object -First 1
if ($null -eq $other) {
    Write-Output "[SKIP] 负坐标副屏：本机只有一块显示器"
} else {
    $wa = $other.WorkingArea
    if (($wa.Width -lt 500) -or ($wa.Height -lt 640)) {
        Write-Output "[SKIP] 负坐标副屏：工作区 $($wa.Width)x$($wa.Height) 放不下窗口"
    } else {
        $tx = [int]($wa.X + [Math]::Floor($wa.Width / 2))
        $ty = [int]($wa.Y + [Math]::Floor($wa.Height / 2))
        $rect = Invoke-SummonAt -X $tx -Y $ty
        $cursor = $script:summonCursor
        $visible = Test-SlipVisible

        Check "副屏上唤出后窗口可见" $visible "visible=$visible"
        if ($null -eq $rect) {
            Check "副屏场景窗口矩形可读" $false "GetWindowRect 失败"
        } else {
            $detail = "screen=$($other.DeviceName) work=($($wa.X),$($wa.Y)) $($wa.Width)x$($wa.Height) " +
                      "win=($($rect.Left),$($rect.Top)) cursor=($($cursor.X),$($cursor.Y))"
            $inside = ($rect.Left -ge $wa.X) -and ($rect.Right -le ($wa.X + $wa.Width)) -and
                      ($rect.Top -ge $wa.Y) -and ($rect.Bottom -le ($wa.Y + $wa.Height))
            Check "窗口完整落在副屏工作区内" $inside $detail

            $covers = ($cursor.X -ge $rect.Left) -and ($cursor.X -le $rect.Right) -and
                      ($cursor.Y -ge $rect.Top) -and ($cursor.Y -le $rect.Bottom)
            Check "副屏场景窗口不覆盖光标" (-not $covers) $detail

            # 只有该屏真的在负坐标区时，这条判据才有意义。
            if (($wa.X -lt 0) -or ($wa.Y -lt 0)) {
                Check "负坐标副屏上窗口未回到主屏正坐标区" `
                    (($rect.Left -lt 0) -or ($rect.Top -lt 0)) $detail
            }
        }
        # 复位到主屏：后续场景仍按主屏坐标判断。
        [void][Win]::SetCursorPos(800, 500)
        Start-Sleep -Milliseconds 400
    }
}

# ----------------------------------------------------------------- 7. 失焦自动收走
$notepad = Start-Process notepad.exe -PassThru
Start-Sleep -Seconds 2
try { $null = [Microsoft.VisualBasic.Interaction]::AppActivate($notepad.Id) } catch {}
Start-Sleep -Seconds 2
$visible = Test-SlipVisible
Check "焦点被其它应用抢走后窗口自动收起" (-not $visible) "visible=$visible"

# ----------------------------------------------------------------- 8. 磁盘
Write-Output "--- inbox.md ---"
$inbox = Join-Path $Vault 'inbox.md'
if (Test-Path $inbox) {
    $content = $utf8.GetString([System.IO.File]::ReadAllBytes($inbox))
    Write-Output $content
    $intact = $content -match '- \[[ x]\] 热键唤出后写入的一条 <!-- slip:v1;id=[0-9a-f-]+;created='
    Check "磁盘上出现完整记录行" $intact ""
    # 只要求 locked=1 落在 marker 内，不绑定字段顺序：
    # rewrite_line 按原行已有的键顺序就地替换，后加的键（如 done_at）会排在末尾。
    Check "锁定标记已落盘" ($content -match '<!-- slip:v1;[^\r\n]*;locked=1[^\r\n]*-->') ""
} else {
    Check "磁盘上出现 inbox.md" $false "文件不存在"
}

Write-Output "--- 启动日志（stderr）---"
# 应用此刻仍在运行、日志文件还被它持有，直接 ReadAllBytes 会撞共享冲突。
try {
    if (Test-Path $errLog) { Write-Output $utf8.GetString([System.IO.File]::ReadAllBytes($errLog)) }
} catch {
    Write-Output "（日志暂时不可读：$($_.Exception.GetType().Name)）"
}

Stop-Slip
Get-Process notepad -ErrorAction SilentlyContinue | Stop-Process -Force

Write-Output ""
if ($script:failed -eq 0) { Write-Output "全部通过。" } else { Write-Output "失败 $script:failed 项。" }
exit $script:failed
