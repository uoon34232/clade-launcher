<#
.SYNOPSIS
  Prints the facts a Clade release test needs to know about the machine. READ-ONLY.

.DESCRIPTION
  Reports: Windows edition/build, architecture, elevation, display scaling (DPI), GPU name/driver/status,
  WebView2 (installed runtime and, when -CladeRoot is given, the bundled one), Java, free disk space and
  the file system of the Clade folder.

  It only reads. It never changes the registry, a device, a driver, the display configuration, the DPI or
  any Windows setting, never calls pnputil/devcon, needs no administrator rights and writes nothing unless
  you pass -OutFile (a plain text copy of the report at the path you choose).

.PARAMETER CladeRoot
  The folder that holds Clade.exe (optional). Used for the drive's free space / file system and for the
  bundled WebView2 check. Paths with spaces and non-ASCII characters are fine.

.PARAMETER OutFile
  Also write the report to this file (UTF-8). Optional.

.EXAMPLE
  powershell -NoProfile -File scripts\qa\environment-report.ps1 -CladeRoot 'D:\Clade-Launcher'
#>
[CmdletBinding()]
param(
  [string] $CladeRoot,
  [string] $OutFile
)

Set-StrictMode -Version 2.0
$ErrorActionPreference = 'Continue'
$lines = New-Object System.Collections.Generic.List[string]
function Add-Line([string] $text) { $lines.Add($text) | Out-Null }
function Add-Item([string] $name, $value) {
  $v = if ($null -eq $value -or "$value" -eq '') { '(unknown)' } else { "$value" }
  Add-Line ('{0,-26} {1}' -f ($name + ':'), $v)
}
function Add-Section([string] $title) { Add-Line ''; Add-Line ('== ' + $title + ' ==') }

Add-Line 'Clade Windows QA environment report (read-only)'
Add-Item 'Generated (local)' (Get-Date -Format 'yyyy-MM-dd HH:mm:ss zzz')

# ---- Windows
Add-Section 'Windows'
try {
  $os = Get-CimInstance Win32_OperatingSystem
  Add-Item 'Edition' $os.Caption
  Add-Item 'Version / build' ('{0} (build {1})' -f $os.Version, $os.BuildNumber)
  Add-Item 'Architecture' $os.OSArchitecture
  Add-Item 'Last boot' $os.LastBootUpTime
} catch { Add-Item 'Windows' ('not readable: ' + $_.Exception.Message) }
try {
  $cv = Get-ItemProperty -LiteralPath 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion' -ErrorAction Stop
  Add-Item 'Display version' $cv.DisplayVersion
  Add-Item 'Revision (UBR)' $cv.UBR
} catch { }
Add-Item 'PowerShell' $PSVersionTable.PSVersion.ToString()
$sandbox = ($env:USERNAME -eq 'WDAGUtilityAccount')
Add-Item 'Inside Windows Sandbox' $sandbox

# ---- Elevation
Add-Section 'User'
try {
  $id = [Security.Principal.WindowsIdentity]::GetCurrent()
  $admin = (New-Object Security.Principal.WindowsPrincipal($id)).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
  Add-Item 'User' $id.Name
  Add-Item 'Elevated (administrator)' $admin
} catch { Add-Item 'Elevation' 'not readable' }

# ---- Display scaling (read-only: GetDpiForSystem / GetDpiForMonitor style queries)
Add-Section 'Display'
try {
  Add-Type -Namespace QaNative -Name Dpi -MemberDefinition @'
[DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
[DllImport("user32.dll")] public static extern uint GetDpiForSystem();
'@ -ErrorAction Stop
  # Only this PowerShell process is made DPI aware so the numbers are real, not virtualised.
  [void][QaNative.Dpi]::SetProcessDPIAware()
  $dpi = [QaNative.Dpi]::GetDpiForSystem()
  Add-Item 'System DPI' ('{0} ({1}% scaling)' -f $dpi, [int][math]::Round($dpi * 100 / 96))
} catch { Add-Item 'System DPI' 'not readable' }
try {
  Add-Type -AssemblyName System.Windows.Forms -ErrorAction Stop
  $i = 0
  foreach ($s in [System.Windows.Forms.Screen]::AllScreens) {
    $i++
    Add-Item ("Monitor $i") ('{0}x{1} at {2},{3}{4}' -f $s.Bounds.Width, $s.Bounds.Height, $s.Bounds.X, $s.Bounds.Y, $(if ($s.Primary) { ' (primary)' } else { '' }))
  }
} catch { Add-Item 'Monitors' 'not readable' }

# ---- GPU (read-only queries only)
Add-Section 'GPU'
try {
  foreach ($g in @(Get-CimInstance Win32_VideoController)) {
    Add-Item 'Adapter' $g.Name
    Add-Item '  Driver version' $g.DriverVersion
    Add-Item '  Driver date' $g.DriverDate
    Add-Item '  Status (WMI)' $g.Status
    Add-Item '  Current resolution' ('{0}x{1}' -f $g.CurrentHorizontalResolution, $g.CurrentVerticalResolution)
  }
} catch { Add-Item 'GPU' ('not readable: ' + $_.Exception.Message) }
try {
  foreach ($d in @(Get-PnpDevice -Class Display -ErrorAction Stop)) {
    Add-Item ('  PnP ' + $d.FriendlyName) ('{0} / {1}' -f $d.Status, $d.Problem)
  }
} catch { Add-Item '  PnP display devices' 'not readable' }
Add-Line 'Note: a Basic Display Adapter or a device in Error / CM_PROB_* state means OpenGL games will not start; re-test after a normal reboot.'

# ---- WebView2
Add-Section 'WebView2'
$wvKeys = @(
  'HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}',
  'HKCU:\Software\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}'
)
$found = $false
foreach ($k in $wvKeys) {
  $pv = $null
  try { $pv = (Get-ItemProperty -LiteralPath $k -Name pv -ErrorAction Stop).pv } catch { }
  if ($pv) { Add-Item ('Installed (' + $k.Substring(0, 4) + ')') $pv; $found = $true }
}
if (-not $found) { Add-Item 'Installed Evergreen runtime' 'none found (a clean machine may have none; Clade ships its own)' }
if ($CladeRoot) {
  $bundled = Join-Path -Path $CladeRoot -ChildPath 'runtimes\webview2\msedgewebview2.exe'
  if (Test-Path -LiteralPath $bundled) {
    $vi = (Get-Item -LiteralPath $bundled).VersionInfo
    Add-Item 'Bundled runtime' ('present, ' + $vi.ProductVersion)
    try { Add-Item 'Bundled signature' (Get-AuthenticodeSignature -LiteralPath $bundled).Status } catch { }
  } else { Add-Item 'Bundled runtime' 'not found under the given Clade folder' }
}

# ---- Java (Clade never uses it; this only tells whether the "no Java installed" premise holds)
Add-Section 'Java on this machine'
$javaOnPath = @()
try { $javaOnPath = @(Get-Command java.exe -All -ErrorAction SilentlyContinue | ForEach-Object { $_.Source }) } catch { }
Add-Item 'java.exe on PATH' $(if ($javaOnPath.Count) { $javaOnPath -join '; ' } else { 'none' })
Add-Item 'JAVA_HOME' $(if ($env:JAVA_HOME) { $env:JAVA_HOME } else { 'not set' })
$pf = @($env:ProgramFiles, ${env:ProgramFiles(x86)}) | Where-Object { $_ }
$javaDirs = @()
foreach ($p in $pf) { foreach ($v in 'Java', 'Eclipse Adoptium', 'Zulu', 'Amazon Corretto') { $d = Join-Path $p $v; if (Test-Path -LiteralPath $d) { $javaDirs += $d } } }
Add-Item 'Program Files Java folders' $(if ($javaDirs.Count) { $javaDirs -join '; ' } else { 'none' })

# ---- Disk and file system
Add-Section 'Disk'
$target = if ($CladeRoot) { $CladeRoot } else { (Get-Location).Path }
try {
  $full = [System.IO.Path]::GetFullPath($target)
  Add-Item 'Checked path' $full
  $root = [System.IO.Path]::GetPathRoot($full)
  if ($root -like '\\*') {
    Add-Item 'Location' 'network/UNC path (not supported for the bundled WebView2)'
  } else {
    $di = New-Object System.IO.DriveInfo($root)
    Add-Item 'Drive' ('{0} ({1})' -f $di.Name, $di.DriveType)
    Add-Item 'File system' $di.DriveFormat
    Add-Item 'Free space' ('{0:N1} GB of {1:N1} GB' -f ($di.AvailableFreeSpace / 1GB), ($di.TotalSize / 1GB))
    if ($di.DriveFormat -in 'FAT32', 'FAT', 'exFAT') { Add-Line 'Note: FAT/exFAT has no ACLs and FAT32 cannot hold files over 4 GB.' }
  }
} catch { Add-Item 'Disk' ('not readable: ' + $_.Exception.Message) }

$text = $lines -join [Environment]::NewLine
Write-Output $text
if ($OutFile) { Set-Content -LiteralPath $OutFile -Value $text -Encoding UTF8 }
