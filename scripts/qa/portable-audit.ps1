<#
.SYNOPSIS
  Read-only before/after audit around a Clade run: did the portable app write anything outside its folder?

.DESCRIPTION
  Two actions:

    -Action Snapshot   records a small, TARGETED picture of the places a portable app must not touch and
                       writes it as JSON into -OutDir (the only thing this script ever writes);
    -Action Compare    compares two snapshots and prints what was added, removed or changed, sorted into
                       EXTERNAL (outside the Clade folder: a problem), INFO (known Windows-owned artifacts,
                       process lists, changes inside the Clade folder) and unattributed noise.

  Snapshot contents: every file under %APPDATA%\.minecraft, %APPDATA% / %LOCALAPPDATA% folders named like
  Clade, and Documents; the top-level folder names of AppData, LocalAppData, ProgramData and the profile;
  the registry values under the Clade keys, the Run keys (autostart) and the Uninstall key NAMES; the
  MuiCache entries that name Clade (Windows writes those itself); the Clade / WebView2 / Java processes;
  and, with -CladeRoot, a size listing of the Clade folder itself.

  It observes only. No registry writes, no device or driver operations, no display / DPI / resolution
  changes, no process injection, no cleanup, and no administrator rights are needed. Paths with spaces and
  non-ASCII characters are handled (every path is used literally).

.PARAMETER Action
  Snapshot or Compare.
.PARAMETER Label
  Snapshot: a short name, for example "before" or "after" (becomes part of the file name).
.PARAMETER OutDir
  Snapshot: folder for the JSON file (created if missing). Default: the current folder.
.PARAMETER CladeRoot
  Snapshot: the Clade folder (the one with Clade.exe), to list it and to tell inside from outside.
.PARAMETER Before / After
  Compare: the two snapshot files.
.PARAMETER MaxFiles
  Safety cap on the number of files listed per location (default 300000).

.EXAMPLE
  .\portable-audit.ps1 -Action Snapshot -Label before -OutDir C:\qa -CladeRoot 'D:\Clade Launcher\Clade-Launcher'
  # ... run Clade, use it, close it ...
  .\portable-audit.ps1 -Action Snapshot -Label after -OutDir C:\qa -CladeRoot 'D:\Clade Launcher\Clade-Launcher'
  .\portable-audit.ps1 -Action Compare -Before C:\qa\snapshot-before-*.json -After C:\qa\snapshot-after-*.json
#>
[CmdletBinding()]
param(
  [Parameter(Mandatory = $true)] [ValidateSet('Snapshot', 'Compare')] [string] $Action,
  [string] $Label = 'snapshot',
  [string] $OutDir = (Get-Location).Path,
  [string] $CladeRoot,
  [string] $Before,
  [string] $After,
  [int] $MaxFiles = 300000
)

Set-StrictMode -Version 2.0
$ErrorActionPreference = 'Continue'

# ------------------------------------------------------------------------------ helpers (read-only)

function Get-FileList([string] $path, [int] $cap) {
  # "path|length|lastWriteTicks" for every file below $path; reparse points (OneDrive placeholders,
  # junctions) are not followed.
  $result = New-Object System.Collections.Generic.List[string]
  if (-not (Test-Path -LiteralPath $path)) { return @() }
  $stack = New-Object System.Collections.Generic.Stack[string]
  $stack.Push($path)
  while ($stack.Count -gt 0 -and $result.Count -lt $cap) {
    $dir = $stack.Pop()
    try {
      foreach ($e in (Get-ChildItem -LiteralPath $dir -Force -ErrorAction SilentlyContinue)) {
        if ($e.Attributes -band [IO.FileAttributes]::ReparsePoint) { continue }
        if ($e.PSIsContainer) { $stack.Push($e.FullName) }
        else { $result.Add(('{0}|{1}|{2}' -f $e.FullName, $e.Length, $e.LastWriteTimeUtc.Ticks)) | Out-Null }
        if ($result.Count -ge $cap) { break }
      }
    } catch { }
  }
  return @($result | Sort-Object)
}

function Get-DirNames([string] $path) {
  if (-not $path -or -not (Test-Path -LiteralPath $path)) { return @() }
  return @(Get-ChildItem -LiteralPath $path -Directory -Force -ErrorAction SilentlyContinue | ForEach-Object { $_.Name } | Sort-Object)
}

function Get-RegistryValues([string] $key) {
  # "name=value" lines for one key (and its subkeys' names); nothing is written.
  if (-not (Test-Path -LiteralPath $key)) { return @() }
  $out = New-Object System.Collections.Generic.List[string]
  try {
    $p = Get-ItemProperty -LiteralPath $key -ErrorAction Stop
    foreach ($n in $p.PSObject.Properties) {
      if ($n.Name -like 'PS*') { continue }
      $out.Add(('{0}={1}' -f $n.Name, $n.Value)) | Out-Null
    }
    foreach ($sub in (Get-ChildItem -LiteralPath $key -ErrorAction SilentlyContinue)) { $out.Add(('[subkey] ' + $sub.PSChildName)) | Out-Null }
  } catch { }
  return @($out | Sort-Object)
}

function Get-RegistryNames([string] $key) {
  if (-not (Test-Path -LiteralPath $key)) { return @() }
  try { return @((Get-Item -LiteralPath $key -ErrorAction Stop).Property | Sort-Object) } catch { return @() }
}

function Get-SubKeyNames([string] $key) {
  if (-not (Test-Path -LiteralPath $key)) { return @() }
  return @(Get-ChildItem -LiteralPath $key -ErrorAction SilentlyContinue | ForEach-Object { $_.PSChildName } | Sort-Object)
}

function New-Snapshot {
  $docs = [Environment]::GetFolderPath('MyDocuments')
  $roaming = $env:APPDATA
  $local = $env:LOCALAPPDATA
  $snap = [ordered]@{}
  $snap.meta = [ordered]@{
    label = $Label
    takenLocal = (Get-Date -Format 'yyyy-MM-dd HH:mm:ss zzz')
    takenUtc = (Get-Date).ToUniversalTime().ToString('o')
    machine = $env:COMPUTERNAME
    user = $env:USERNAME
    cladeRoot = $CladeRoot
    elevated = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
  }
  # Files in places that must stay free of launcher state.
  $files = [ordered]@{}
  $files['APPDATA\.minecraft'] = Get-FileList (Join-Path $roaming '.minecraft') $MaxFiles
  foreach ($base in @(@('APPDATA', $roaming), @('LOCALAPPDATA', $local))) {
    foreach ($d in (Get-DirNames $base[1] | Where-Object { $_ -match '(?i)clade|dev\.clade|minecraft' })) {
      $files[('{0}\{1}' -f $base[0], $d)] = Get-FileList (Join-Path $base[1] $d) $MaxFiles
    }
  }
  $files['Documents'] = Get-FileList $docs $MaxFiles
  $snap.files = $files
  # Folder names only (cheap, and shows a new "Clade" / WebView2 / Mojang folder appearing).
  $snap.topLevelFolders = [ordered]@{
    'APPDATA' = Get-DirNames $roaming
    'LOCALAPPDATA' = Get-DirNames $local
    'ProgramData' = Get-DirNames $env:ProgramData
    'USERPROFILE' = Get-DirNames $env:USERPROFILE
  }
  # Registry (read only).
  $reg = [ordered]@{}
  foreach ($k in 'HKCU:\Software\Clade', 'HKCU:\Software\dev.clade.launcher', 'HKCU:\Software\Clade Launcher') { $reg[$k] = Get-RegistryValues $k }
  $reg['HKCU Run (autostart)'] = Get-RegistryValues 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
  $reg['HKCU RunOnce (autostart)'] = Get-RegistryValues 'HKCU:\Software\Microsoft\Windows\CurrentVersion\RunOnce'
  $reg['HKCU Uninstall (key names)'] = Get-SubKeyNames 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall'
  $snap.registry = $reg
  # Windows writes MuiCache / app-compat entries for every exe it has seen; they are informational.
  $mui = Get-RegistryNames 'HKCU:\Software\Classes\Local Settings\Software\Microsoft\Windows\Shell\MuiCache'
  $snap.windowsOwnedArtifacts = [ordered]@{
    'MuiCache entries naming Clade' = @($mui | Where-Object { $_ -match '(?i)clade' })
  }
  # Processes.
  $procs = @()
  try {
    $procs = @(Get-CimInstance Win32_Process -ErrorAction Stop | Where-Object { $_.Name -match '(?i)^(clade|msedgewebview2|java|javaw)\.exe$' } |
        ForEach-Object { ('{0} pid={1} {2}' -f $_.Name, $_.ProcessId, $_.ExecutablePath) } | Sort-Object)
  } catch { }
  $snap.processes = $procs
  # The Clade folder itself: relative path + size (no contents), to show what Clade created inside it.
  if ($CladeRoot -and (Test-Path -LiteralPath $CladeRoot)) {
    $rootFull = (Resolve-Path -LiteralPath $CladeRoot).Path.TrimEnd('\')
    $rel = foreach ($line in (Get-FileList $rootFull $MaxFiles)) {
      $parts = $line -split '\|'
      '{0}|{1}' -f $parts[0].Substring($rootFull.Length).TrimStart('\'), $parts[1]
    }
    $snap.cladeRootFiles = @($rel | Where-Object { $_ -notmatch '^(runtimes\\webview2|runtimes\\java|assets|libraries|minecraft|cache)\\' })
    $snap.cladeRootBigFolders = [ordered]@{}
    foreach ($b in 'runtimes\webview2', 'runtimes\java', 'assets', 'libraries', 'minecraft', 'cache') {
      $p = Join-Path $rootFull $b
      $n = 0; $bytes = 0
      if (Test-Path -LiteralPath $p) {
        foreach ($f in (Get-ChildItem -LiteralPath $p -Recurse -Force -File -ErrorAction SilentlyContinue)) { $n++; $bytes += $f.Length }
      }
      $snap.cladeRootBigFolders[$b] = ('{0} files, {1:N1} MB' -f $n, ($bytes / 1MB))
    }
  }
  return $snap
}

function Convert-ToSet($value) {
  # An empty registry key is stored as {}; it must become an empty list, not one blank entry (which
  # the comparison would report as a blank REMOVED/CHANGED row).
  return @($value | ForEach-Object { "$_" } | Where-Object { $_.Trim() -ne '' })
}

function Test-EdgeAutoLaunch([string] $line) {
  # Microsoft Edge registers its own auto-launch Run value after a browser sign-in (Clade opens the system
  # browser for Microsoft OAuth). Only the exact Edge shape in a normal Program Files install is browser-owned:
  # the value name must be MicrosoftEdgeAutoLaunch_<32 hex>, and the value must be the quoted msedge.exe
  # (optionally with the two switches Edge itself adds). Anything else stays a finding.
  return ($line -match '^MicrosoftEdgeAutoLaunch_[0-9A-Fa-f]{32}="[A-Za-z]:\\Program Files( \(x86\))?\\Microsoft\\Edge\\Application\\msedge\.exe"( --(no-startup-window|win-session-start))*$')
}

function Compare-Lists([string[]] $a, [string[]] $b, [scriptblock] $key) {
  # Returns added / removed / changed, matching entries by $key (the part before the first '|').
  $ma = @{}; foreach ($x in $a) { $ma[(& $key $x)] = $x }
  $mb = @{}; foreach ($x in $b) { $mb[(& $key $x)] = $x }
  $added = @($mb.Keys | Where-Object { -not $ma.ContainsKey($_) } | Sort-Object)
  $removed = @($ma.Keys | Where-Object { -not $mb.ContainsKey($_) } | Sort-Object)
  $changed = @($mb.Keys | Where-Object { $ma.ContainsKey($_) -and $ma[$_] -ne $mb[$_] } | Sort-Object)
  return [pscustomobject]@{ Added = $added; Removed = $removed; Changed = $changed }
}

function Read-Snapshot([string] $path) {
  $resolved = @(if (Test-Path -LiteralPath $path) {
    Resolve-Path -LiteralPath $path -ErrorAction Stop
  } else {
    Resolve-Path -Path $path -ErrorAction Stop
  })
  if ($resolved.Count -ne 1) { throw "Expected exactly one snapshot for '$path', found $($resolved.Count)." }
  return Get-Content -LiteralPath $resolved[0].Path -Raw -Encoding UTF8 | ConvertFrom-Json
}

# ------------------------------------------------------------------------------------------ actions

if ($Action -eq 'Snapshot') {
  if (-not (Test-Path -LiteralPath $OutDir)) { New-Item -ItemType Directory -Path $OutDir -Force | Out-Null }
  $snap = New-Snapshot
  $stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
  $file = Join-Path $OutDir ('snapshot-{0}-{1}.json' -f ($Label -replace '[^A-Za-z0-9._-]', '_'), $stamp)
  ($snap | ConvertTo-Json -Depth 6) | Set-Content -LiteralPath $file -Encoding UTF8
  Write-Output ('Snapshot written: ' + $file)
  $count = 0; foreach ($k in $snap.files.Keys) { $count += @($snap.files[$k]).Count }
  Write-Output ('  files listed in watched locations: {0}; processes: {1}' -f $count, @($snap.processes).Count)
  return
}

# ---- Compare
if (-not $Before -or -not $After) { throw 'Compare needs -Before and -After.' }
$b = Read-Snapshot $Before
$a = Read-Snapshot $After
$external = New-Object System.Collections.Generic.List[string]
$info = New-Object System.Collections.Generic.List[string]
$noise = New-Object System.Collections.Generic.List[string]

Write-Output ('Before: {0}  ({1})' -f $b.meta.takenLocal, $b.meta.label)
Write-Output ('After:  {0}  ({1})' -f $a.meta.takenLocal, $a.meta.label)

# Files in watched locations.
$locations = @(@($b.files.PSObject.Properties.Name) + @($a.files.PSObject.Properties.Name) | Select-Object -Unique)
foreach ($loc in $locations) {
  $la = if ($b.files.PSObject.Properties[$loc]) { Convert-ToSet $b.files.$loc } else { @() }
  $lb = if ($a.files.PSObject.Properties[$loc]) { Convert-ToSet $a.files.$loc } else { @() }
  $d = Compare-Lists $la $lb { param($x) ($x -split '\|')[0] }
  $target = $external
  if ($loc -eq 'Documents') { $target = $noise }
  foreach ($x in $d.Added) { $target.Add("[$loc] ADDED   $x") | Out-Null }
  foreach ($x in $d.Removed) { $target.Add("[$loc] REMOVED $x") | Out-Null }
  foreach ($x in $d.Changed) { $target.Add("[$loc] CHANGED $x") | Out-Null }
}
# Folders.
foreach ($area in @($a.topLevelFolders.PSObject.Properties.Name)) {
  $fb = Convert-ToSet $b.topLevelFolders.$area
  $fa = Convert-ToSet $a.topLevelFolders.$area
  foreach ($n in ($fa | Where-Object { $fb -notcontains $_ })) {
    $line = "[$area] new top-level folder: $n"
    if ($n -match '(?i)clade|minecraft|webview|launcher|mojang|java') { $external.Add($line) | Out-Null } else { $noise.Add($line) | Out-Null }
  }
}
# Registry.
foreach ($k in @($a.registry.PSObject.Properties.Name)) {
  $rb = Convert-ToSet $b.registry.$k
  $ra = Convert-ToSet $a.registry.$k
  foreach ($x in ($ra | Where-Object { $rb -notcontains $_ })) {
    if ($k -eq 'HKCU Run (autostart)' -and (Test-EdgeAutoLaunch $x)) { $info.Add("[browser-owned] Microsoft Edge auto-launch ADDED/CHANGED $x") | Out-Null }
    else { $external.Add("[registry $k] ADDED/CHANGED $x") | Out-Null }
  }
  foreach ($x in ($rb | Where-Object { $ra -notcontains $_ })) {
    if ($k -eq 'HKCU Run (autostart)' -and (Test-EdgeAutoLaunch $x)) { $info.Add("[browser-owned] Microsoft Edge auto-launch REMOVED/CHANGED $x") | Out-Null }
    else { $external.Add("[registry $k] REMOVED/CHANGED $x") | Out-Null }
  }
}
# Windows-owned artifacts (informational).
foreach ($k in @($a.windowsOwnedArtifacts.PSObject.Properties.Name)) {
  $wb = Convert-ToSet $b.windowsOwnedArtifacts.$k
  foreach ($x in (Convert-ToSet $a.windowsOwnedArtifacts.$k | Where-Object { $wb -notcontains $_ })) { $info.Add("[Windows-owned] $k : $x") | Out-Null }
}
# Processes.
$pb = Convert-ToSet $b.processes
foreach ($x in (Convert-ToSet $a.processes | Where-Object { $pb -notcontains $_ })) { $info.Add("[process started] $x") | Out-Null }
$pa = Convert-ToSet $a.processes
foreach ($x in ($pb | Where-Object { $pa -notcontains $_ })) { $info.Add("[process gone]    $x") | Out-Null }
# Inside the Clade folder (informational: this is where its state belongs).
if ($a.PSObject.Properties['cladeRootFiles']) {
  $ib = if ($b.PSObject.Properties['cladeRootFiles']) { Convert-ToSet $b.cladeRootFiles } else { @() }
  $d = Compare-Lists $ib (Convert-ToSet $a.cladeRootFiles) { param($x) ($x -split '\|')[0] }
  foreach ($x in $d.Added) { $info.Add("[inside Clade folder] ADDED   $x") | Out-Null }
  foreach ($x in $d.Removed) { $info.Add("[inside Clade folder] REMOVED $x") | Out-Null }
  if ($a.PSObject.Properties['cladeRootBigFolders']) {
    foreach ($n in @($a.cladeRootBigFolders.PSObject.Properties.Name)) {
      $before = if ($b.PSObject.Properties['cladeRootBigFolders'] -and $b.cladeRootBigFolders.PSObject.Properties[$n]) { $b.cladeRootBigFolders.$n } else { '(none)' }
      if ($before -ne $a.cladeRootBigFolders.$n) { $info.Add("[inside Clade folder] $n : $before -> $($a.cladeRootBigFolders.$n)") | Out-Null }
    }
  }
}

function Write-Group([string] $title, $items) {
  Write-Output ''
  Write-Output ('== {0} ({1}) ==' -f $title, @($items).Count)
  foreach ($x in $items) { Write-Output ('  ' + $x) }
}
Write-Group 'EXTERNAL changes: outside the Clade folder (must be empty)' $external
Write-Group 'INFO: Windows-owned artifacts, processes, changes inside the Clade folder' $info
Write-Group 'UNATTRIBUTED: other applications may have caused these (Documents, unrelated new folders)' $noise
Write-Output ''
if ($external.Count -eq 0) { Write-Output 'RESULT: no change outside the Clade folder in the watched locations.'; exit 0 }
Write-Output 'RESULT: CHANGES OUTSIDE THE CLADE FOLDER - see EXTERNAL above.'
exit 1
