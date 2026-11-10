﻿# 窗口诊断：Slip 在跑但看不到窗口时，先跑这个。
#
# 输出：进程、窗口可见/最小化/被隐藏(cloaked)、物理与虚拟矩形、所在显示器、
#       前台归属、该进程全部顶层窗口，以及对窗口矩形做一次像素取样
#       （纯色 = 界面没画出来；多彩 = 画出来了，问题在别处）。
#
# 用法：
#   pwsh -File tools/diag-window.ps1              # 自动找 slip-shell
#   pwsh -File tools/diag-window.ps1 -TargetPid 1234
#
# 参数不能叫 -Pid：`$Pid` 是 PowerShell 的只读自动变量。
param([int]$TargetPid = 0)

$PSNativeCommandUseErrorActionPreference = $false
Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName System.Windows.Forms

Add-Type -TypeDefinition @"
using System;
using System.Text;
using System.Collections.Generic;
using System.Runtime.InteropServices;

public static class WinDiag {
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }

    public delegate bool EnumProc(IntPtr h, IntPtr p);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr p);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetClassName(IntPtr h, StringBuilder s, int c);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int c);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr h);
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] public static extern int GetWindowLong(IntPtr h, int index);
    [DllImport("dwmapi.dll")] public static extern int DwmGetWindowAttribute(IntPtr h, int attr, out int value, int size);

    public const int GWL_STYLE = -16;
    public const int GWL_EXSTYLE = -20;
    public const int DWMWA_CLOAKED = 14;

    public static string Cls(IntPtr h) { var s = new StringBuilder(256); GetClassName(h, s, 256); return s.ToString(); }
    public static string Txt(IntPtr h) { var s = new StringBuilder(512); GetWindowTextW(h, s, 512); return s.ToString(); }
    public static uint PidOf(IntPtr h) { uint pid; GetWindowThreadProcessId(h, out pid); return pid; }

    /// 被 Windows 遮蔽：最常见的来源是"窗口在另一个虚拟桌面上"。
    public static int Cloaked(IntPtr h) {
        int v;
        return DwmGetWindowAttribute(h, DWMWA_CLOAKED, out v, sizeof(int)) == 0 ? v : -1;
    }

    public static List<IntPtr> TopWindows(uint want) {
        var list = new List<IntPtr>();
        EnumWindows((h, p) => { if (PidOf(h) == want) list.Add(h); return true; }, IntPtr.Zero);
        return list;
    }

    public static IntPtr SlipWindow(uint want) {
        foreach (var h in TopWindows(want)) { if (Cls(h) == "Tauri Window") return h; }
        return IntPtr.Zero;
    }
}
"@

# 找出要诊断的实例。**默认检查全部 slip-shell 进程**，因为单实例插件存在时
# 很容易出现"一个实例只负责转发、另一个才持有窗口"的局面；只看第一个会误判成
# "没有窗口"。
$all = if ($TargetPid -gt 0) {
    @(Get-Process -Id $TargetPid -ErrorAction SilentlyContinue)
} else {
    @(Get-Process slip-shell -ErrorAction SilentlyContinue)
}
if ($all.Count -eq 0) {
    Write-Output "没找到 slip-shell 进程。先启动它，或用 -TargetPid 指定。"
    exit 1
}

Write-Output ("发现 slip-shell 实例数: " + $all.Count)
foreach ($proc in $all) {
    $wins = [WinDiag]::TopWindows([uint32]$proc.Id)
    $hasTauri = ([WinDiag]::SlipWindow([uint32]$proc.Id) -ne [IntPtr]::Zero)
    Write-Output ("  PID=" + $proc.Id + " 启动=" + $proc.StartTime + " 顶层窗口=" + $wins.Count + " 有Tauri窗口=" + $hasTauri + " 会话=" + $proc.SessionId)
}
Write-Output ""

if ($all.Count -gt 1) {
    Write-Output "注意：**同时存在多个实例**。单实例插件只让第一个持有窗口，其余是转发进程——"
    Write-Output "      你看到的『没有窗口』可能就是因为热键/双击被转发到了那个真正持有窗口的实例。"
    Write-Output ""
}

$target = $all[0]
$procId = [uint32]$target.Id
Write-Output ("详查 PID=" + $procId + " 启动于 " + $target.StartTime)
Write-Output ""

$windows = [WinDiag]::TopWindows($procId)
Write-Output ("该进程顶层窗口数: " + $windows.Count)
foreach ($h in $windows) {
    $r = New-Object WinDiag+RECT
    [void][WinDiag]::GetWindowRect($h, [ref]$r)
    $cloaked = [WinDiag]::Cloaked($h)
    Write-Output ("  " + [WinDiag]::Cls($h) + "  title='" + [WinDiag]::Txt($h) + "'  hwnd=" + $h.ToInt64())
    Write-Output ("      visible=" + [WinDiag]::IsWindowVisible($h) + " minimized=" + [WinDiag]::IsIconic($h) + " cloaked=" + $cloaked)
    Write-Output ("      rect(virtual)=" + $r.Left + "," + $r.Top + " 尺寸 " + ($r.Right - $r.Left) + "x" + ($r.Bottom - $r.Top))
}

# 会话与线程：区分"窗口没建出来"和"建在别的会话/桌面上"。
$mySession = (Get-Process -Id $PID).SessionId
$targetThreads = $target.Threads
$waitSummary = $targetThreads | Group-Object ThreadState | ForEach-Object { $_.Name + "=" + $_.Count }
Write-Output ""
Write-Output ("会话：目标进程=" + $target.SessionId + "  本脚本=" + $mySession +
    "  " + $(if ($target.SessionId -eq $mySession) { "（同一会话）" } else { "（**不同会话**：窗口在另一个登录会话里，你看不到是正常的）" }))
Write-Output ("线程数=" + $targetThreads.Count + "  状态: " + ($waitSummary -join ", "))
Write-Output ""

$h = [WinDiag]::SlipWindow($procId)
if ($h -eq [IntPtr]::Zero) {
    Write-Output "结论：进程活着，但**没有 Tauri 窗口**（连托盘和热键插件的隐藏窗口都没有）。"
    Write-Output ""
    Write-Output "两种可能，按下面两条区分："
    Write-Output "  1) 卡在建窗口之前 —— 应用自己的 stdout 会停在中途，看不到 '[slip] window report'。"
    Write-Output "  2) 窗口建在别的会话/桌面上 —— 上面『会话』一行会显示不同会话。"
    Write-Output ""
    Write-Output "请把**同一个 PID** 那次启动的完整 stdout/stderr 贴出来："
    Write-Output "  cargo run -p slip-shell 2>&1 | Tee-Object -FilePath run.log"
    exit 2
}

$r = New-Object WinDiag+RECT
[void][WinDiag]::GetWindowRect($h, [ref]$r)
$cloaked = [WinDiag]::Cloaked($h)
$style = [WinDiag]::GetWindowLong($h, [WinDiag]::GWL_STYLE)
$exstyle = [WinDiag]::GetWindowLong($h, [WinDiag]::GWL_EXSTYLE)
$fg = [WinDiag]::GetForegroundWindow()

Write-Output "=== 主窗口 ==="
Write-Output ("visible=" + [WinDiag]::IsWindowVisible($h) + "  WS_VISIBLE=" + (($style -band 0x10000000) -ne 0))
Write-Output ("minimized=" + [WinDiag]::IsIconic($h) + "  WS_EX_TOOLWINDOW=" + (($exstyle -band 0x80) -ne 0))
Write-Output ("cloaked=" + $cloaked + "   （1 = 在另一个虚拟桌面上，或已被系统隐藏）")
Write-Output ("前台是这个窗口吗: " + ([WinDiag]::PidOf($fg) -eq $procId) + "   前台窗口类=" + [WinDiag]::Cls($fg) + " 标题='" + [WinDiag]::Txt($fg) + "'")

# 虚拟坐标 → 物理坐标（脚本是 DPI 不感知进程，本机 150%）
$scale = 1.5
$px = [int]($r.Left * $scale); $py = [int]($r.Top * $scale)
$pw = [int](($r.Right - $r.Left) * $scale); $ph = [int](($r.Bottom - $r.Top) * $scale)
Write-Output ("rect(physical)=" + $px + "," + $py + " 尺寸 " + $pw + "x" + $ph + "  （按缩放 " + $scale + " 折算）")

# 落在哪个显示器上
$cx = $r.Left + [int](($r.Right - $r.Left) / 2)
$cy = $r.Top + [int](($r.Bottom - $r.Top) / 2)
foreach ($s in [System.Windows.Forms.Screen]::AllScreens) {
    $b = $s.Bounds
    if ($cx -ge $b.X -and $cx -le ($b.X + $b.Width) -and $cy -ge $b.Y -and $cy -le ($b.Y + $b.Height)) {
        Write-Output ("窗口中心落在显示器: " + $s.DeviceName + "  primary=" + $s.Primary + "  bounds=" + $b)
    }
}

Write-Output ""
Write-Output "=== 像素取样（该矩形在屏幕上的实际内容）==="
if ($pw -le 0 -or $ph -le 0) { Write-Output "矩形无效，跳过"; exit 0 }
$bmp = New-Object System.Drawing.Bitmap $pw, $ph
$g = [System.Drawing.Graphics]::FromImage($bmp)
try {
    $g.CopyFromScreen($px, $py, 0, 0, (New-Object System.Drawing.Size $pw, $ph))
} catch {
    Write-Output ("截图失败: " + $_.Exception.Message)
    exit 0
}
$colors = @{}
for ($y = 0; $y -lt $bmp.Height; $y += 8) {
    for ($x = 0; $x -lt $bmp.Width; $x += 8) {
        $c = $bmp.GetPixel($x, $y)
        $k = "" + $c.R + "," + $c.G + "," + $c.B
        if ($colors.ContainsKey($k)) { $colors[$k]++ } else { $colors[$k] = 1 }
    }
}
$g.Dispose(); $bmp.Dispose()

Write-Output ("不同颜色数=" + $colors.Count)
foreach ($t in ($colors.GetEnumerator() | Sort-Object Value -Descending | Select-Object -First 3)) {
    Write-Output ("  " + $t.Key + "  x" + $t.Value)
}
Write-Output ""
if ($colors.Count -le 2) {
    Write-Output "结论：该区域是纯色 → 界面没有画到屏幕上（或窗口被别的东西盖住/在另一个桌面）。"
    Write-Output "      若纸色(约 250,247,239) 占绝对多数，说明窗口在、但内容是空白。"
} else {
    Write-Output "结论：该区域有实际画面 → 界面确实画在屏幕上了。"
    Write-Output "      如果你肉眼看不到，检查：是否在另一个虚拟桌面(cloaked=1)、"
    Write-Output "      是否看的是另一台显示器、或窗口被全屏应用盖住。"
}
