<#
.SYNOPSIS
  Smoke test of a portable package: validate it, extract it, start Clade from the extracted folder and
  check the result. Runs locally and in CI; it needs no sign-in, no network and no account.

.DESCRIPTION
  Checks, in order:
   - the .sha256 file next to the ZIP matches the ZIP;
   - the package passes the allowlist validation (no user data, no development files, licences present);
   - the ZIP extracts (into a folder whose name contains a space);
   - Clade.exe's version equals -ExpectedVersion; LICENSE, THIRD-PARTY-NOTICES.txt and the WebView2 folder exist;
   - nothing but the release files is there before the first start;
   - Clade starts and keeps running, the portable folders are created BESIDE Clade.exe (which is also how a
     release build, as opposed to a debug build, behaves), the log says the bundled WebView2 runtime is used,
     and every WebView2 process of this Clade runs from the package's runtimes\webview2 folder;
   - nothing connects to the Vite development port (a canary listener sits on 127.0.0.1:1420 during the run).

.PARAMETER Zip
  The Clade-Launcher-Portable.zip to test.
.PARAMETER ExpectedVersion
  The release version (for example 0.1.0).
.PARAMETER WithoutWebView2
  For a development package only: accept that the system's WebView2 is used.
.PARAMETER StartSeconds
  How long to let Clade run before checking (default 15).
#>
[CmdletBinding()]
param(
  [Parameter(Mandatory = $true)] [string] $Zip,
  [Parameter(Mandatory = $true)] [string] $ExpectedVersion,
  [switch] $WithoutWebView2,
  [int] $StartSeconds = 15,
  [switch] $KeepFolder
)

$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$zipPath = (Resolve-Path $Zip).Path
$failures = New-Object System.Collections.Generic.List[string]
function Check($ok, $what) { if ($ok) { Write-Host "  ok    $what" } else { Write-Host "  FAIL  $what"; $failures.Add($what) } }

Write-Host "Smoke test: $zipPath"

# ---- the checksum file
$sidecar = "$zipPath.sha256"
$hash = (Get-FileHash $zipPath -Algorithm SHA256).Hash.ToLower()
$expected = if (Test-Path $sidecar) { ((Get-Content $sidecar -Raw) -split '\s+')[0].ToLower() } else { '' }
Check ($expected -eq $hash) 'the .sha256 file matches the ZIP'

# ---- the allowlist
$valArgs = @((Join-Path $repo 'scripts\release\validate-package.mjs'), $zipPath, '--version', $ExpectedVersion, '--csp-from', (Join-Path $repo 'src-tauri\tauri.conf.json'))
if ($WithoutWebView2) { $valArgs += '--allow-missing-webview2' }
& node @valArgs
Check ($LASTEXITCODE -eq 0) 'the package passes the allowlist validation'

# ---- extract into a folder with a space in its name
$root = Join-Path ([System.IO.Path]::GetTempPath()) ('Clade smoke ' + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Path $root | Out-Null
$process = $null
$canary = $null
try {
  Add-Type -AssemblyName System.IO.Compression.FileSystem
  [System.IO.Compression.ZipFile]::ExtractToDirectory($zipPath, $root)
  $dir = Join-Path $root 'Clade-Launcher'
  Check (Test-Path (Join-Path $dir 'Clade.exe')) 'Clade.exe is at the package root after extraction'
  if (-not (Test-Path (Join-Path $dir 'Clade.exe'))) { throw 'no Clade.exe' }
  $exe = Join-Path $dir 'Clade.exe'

  $v = (Get-Item $exe).VersionInfo
  Check ($v.ProductVersion -eq $ExpectedVersion -and $v.FileVersion -eq $ExpectedVersion) "Clade.exe version is $ExpectedVersion (found $($v.ProductVersion))"
  Check ($v.ProductName -eq 'Clade Launcher') 'the product name is Clade Launcher'
  Check (Test-Path (Join-Path $dir 'LICENSE')) 'LICENSE is present'
  Check (Test-Path (Join-Path $dir 'THIRD-PARTY-NOTICES.txt')) 'THIRD-PARTY-NOTICES.txt is present'
  if (-not $WithoutWebView2) { Check (Test-Path (Join-Path $dir 'runtimes\webview2\msedgewebview2.exe')) 'the bundled WebView2 runtime is present' }

  $userData = 'data', 'instances', 'logs', 'cache', 'minecraft', 'libraries', 'assets', 'worlds'
  $present = $userData | Where-Object { Test-Path (Join-Path $dir $_) }
  Check (-not $present) 'no user data folders are in the package before the first start'
  $loose = Get-ChildItem $dir -Force -File | Where-Object { $_.Name -notin 'Clade.exe', 'README.txt', 'LICENSE', 'THIRD-PARTY-NOTICES.txt' }
  Check (-not $loose) 'no other loose files are in the package'

  # ---- start it, with a canary where a development server would be
  try {
    $canary = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, 1420)
    $canary.Start()
  } catch { $canary = $null; Write-Host '  note  port 1420 is in use; the development-server canary was skipped' }
  $process = Start-Process -FilePath $exe -WorkingDirectory $dir -PassThru
  Start-Sleep -Seconds $StartSeconds
  Check (-not $process.HasExited) 'Clade is still running after the start'

  foreach ($d in 'data', 'instances', 'runtimes\java', 'minecraft', 'libraries', 'assets', 'cache', 'logs') {
    Check (Test-Path (Join-Path $dir $d)) "the portable folder $d was created beside Clade.exe"
  }
  Check (Test-Path (Join-Path $dir 'data\clade.lock')) 'the folder lock exists'
  Check (-not (Test-Path (Join-Path $dir 'clade-root'))) 'no debug-style clade-root folder (this is a release build)'

  $logFile = Join-Path $dir 'logs\clade.log'
  $log = if (Test-Path $logFile) { Get-Content $logFile -Raw } else { '' }
  Check ($log -match 'launcher starting') 'the log says the launcher started'
  if ($WithoutWebView2) { Check ($log -match 'webview=System') 'the log says the system WebView2 is used (development package)' }
  else { Check ($log -match 'webview=Bundled') 'the log says the BUNDLED WebView2 runtime is used' }
  Check ($log -notmatch 'ERROR') 'the log has no errors'

  if (-not $WithoutWebView2) {
    $wvRoot = (Join-Path $dir 'runtimes\webview2')
    $children = Get-CimInstance Win32_Process -Filter "Name = 'msedgewebview2.exe'" | Where-Object { $_.ExecutablePath }
    $mine = $children | Where-Object { $_.ExecutablePath.StartsWith($wvRoot, [System.StringComparison]::OrdinalIgnoreCase) }
    $foreignUnderClade = $children | Where-Object { -not $_.ExecutablePath.StartsWith($wvRoot, [System.StringComparison]::OrdinalIgnoreCase) -and $_.CommandLine -like "*$dir*" }
    Check (@($mine).Count -gt 0) 'WebView2 processes are running from the package runtimes\webview2 folder'
    Check (-not $foreignUnderClade) 'no other WebView2 runtime is used for Clade'
  }

  if ($canary) {
    Check (-not $canary.Pending()) 'nothing tried to reach the Vite development server (127.0.0.1:1420)'
  }
} finally {
  if ($canary) { $canary.Stop() }
  Get-Process -Name 'Clade' -ErrorAction SilentlyContinue | Where-Object { $_.Path -and $_.Path.StartsWith($root, [System.StringComparison]::OrdinalIgnoreCase) } | Stop-Process -Force
  Start-Sleep -Milliseconds 1500
  if (-not $KeepFolder) { Remove-Item $root -Recurse -Force -ErrorAction SilentlyContinue } else { Write-Host "  kept  $root" }
}

if ($failures.Count) {
  Write-Host ''
  Write-Host "SMOKE TEST FAILED ($($failures.Count)):"
  $failures | ForEach-Object { Write-Host "  - $_" }
  exit 1
}
Write-Host ''
Write-Host 'Smoke test passed.'
