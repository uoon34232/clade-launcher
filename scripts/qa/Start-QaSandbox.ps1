<#
.SYNOPSIS
  Stages a Clade portable ZIP and starts it in a clean Windows Sandbox. Never enables any Windows feature.

.DESCRIPTION
  1. Checks the ZIP and copies it (and its .sha256, when present) into a NEW staging folder under %TEMP%.
  2. Copies the QA scripts next to it.
  3. Writes a resolved copy of the chosen .wsb template (absolute staging / results folders substituted).
  4. If Windows Sandbox is installed, starts it; otherwise prints how to turn the feature on yourself and
     stops. It never enables a Windows feature, never touches Hyper-V, virtualization or the BIOS.

  What the sandbox can see: ONLY the staging folder (READ-ONLY) and one empty results folder (writable).
  The repository, the user profile, the real .minecraft and any real Clade data are never shared.

.PARAMETER Zip
  The Clade-Launcher-Portable*.zip to test.
.PARAMETER Mode
  offline (no network, no virtual GPU), online (network, no virtual GPU) or online-gpu (network + virtual
  GPU; needed to actually open a Minecraft window).
.PARAMETER StagingRoot
  Parent folder for the staging folder (default: %TEMP%). Must not be a profile / AppData / .minecraft folder.
.PARAMETER DryRun
  Do everything except starting the sandbox (stage, resolve, print).
#>
[CmdletBinding()]
param(
  [Parameter(Mandatory = $true)] [string] $Zip,
  [ValidateSet('offline', 'online', 'online-gpu')] [string] $Mode = 'offline',
  [string] $StagingRoot = $env:TEMP,
  [switch] $DryRun
)

Set-StrictMode -Version 2.0
$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..\..')).Path
$templateDir = Join-Path $repo 'qa\windows-sandbox'

if (-not (Test-Path -LiteralPath $Zip -PathType Leaf)) { throw "ZIP not found: $Zip" }
$zipFull = (Resolve-Path -LiteralPath $Zip).Path
if ((Split-Path -Leaf $zipFull) -notlike 'Clade-Launcher-Portable*.zip') { throw 'The file name must look like Clade-Launcher-Portable*.zip' }

# The staging folder must be a fresh folder of its own, nowhere near real user data.
$rootFull = [System.IO.Path]::GetFullPath($StagingRoot).TrimEnd('\')
foreach ($forbidden in @($env:USERPROFILE, $env:APPDATA, $env:LOCALAPPDATA, (Join-Path $env:APPDATA '.minecraft'), [Environment]::GetFolderPath('MyDocuments'))) {
  if ($forbidden -and ($rootFull -ieq $forbidden.TrimEnd('\'))) { throw "Refusing to stage directly in $forbidden. Use a temp folder." }
}
if ($rootFull -like ($repo.TrimEnd('\') + '*')) { throw 'Refusing to stage inside the repository.' }

$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$staging = Join-Path $rootFull ('clade-qa-' + $stamp)
$inDir = Join-Path $staging 'in'
$qaDir = Join-Path $inDir 'qa'
$results = Join-Path $staging 'results'
Write-Output ('Mode:     ' + $Mode)
Write-Output ('Package:  ' + $zipFull)
Write-Output ('Staging:  ' + $staging + '   (shared READ-ONLY as C:\CladeQA\in)')
Write-Output ('Results:  ' + $results + '   (shared writable as C:\CladeQA\results; empty)')

New-Item -ItemType Directory -Path $qaDir -Force | Out-Null
New-Item -ItemType Directory -Path $results -Force | Out-Null
Copy-Item -LiteralPath $zipFull -Destination $inDir
$sum = $zipFull + '.sha256'
if (Test-Path -LiteralPath $sum) { Copy-Item -LiteralPath $sum -Destination $inDir }
else {
  $h = (Get-FileHash -LiteralPath $zipFull -Algorithm SHA256).Hash.ToLowerInvariant()
  Set-Content -LiteralPath (Join-Path $inDir ((Split-Path -Leaf $zipFull) + '.sha256')) -Value ('{0}  {1}' -f $h, (Split-Path -Leaf $zipFull)) -Encoding ASCII
  Write-Output ('No .sha256 beside the ZIP; computed on the host: ' + $h)
}
foreach ($f in 'environment-report.ps1', 'portable-audit.ps1') { Copy-Item -LiteralPath (Join-Path $PSScriptRoot $f) -Destination $qaDir }
Copy-Item -LiteralPath (Join-Path $templateDir 'sandbox-run.ps1') -Destination $qaDir

$template = Join-Path $templateDir ('clade-' + $Mode + '.wsb')
$text = Get-Content -LiteralPath $template -Raw -Encoding UTF8
$text = $text.Replace('%CLADE_QA_STAGING%', [System.Security.SecurityElement]::Escape($inDir)).Replace('%CLADE_QA_RESULTS%', [System.Security.SecurityElement]::Escape($results))
$wsb = Join-Path $staging ('clade-' + $Mode + '.wsb')
Set-Content -LiteralPath $wsb -Value $text -Encoding UTF8
[void][xml](Get-Content -LiteralPath $wsb -Raw -Encoding UTF8)   # must be well-formed XML
Write-Output ('Config:   ' + $wsb)

$sandboxExe = Join-Path $env:SystemRoot 'System32\WindowsSandbox.exe'
if (-not (Test-Path -LiteralPath $sandboxExe)) {
  Write-Output ''
  Write-Output 'Windows Sandbox is NOT installed on this PC, so nothing was started. This script does not enable it.'
  Write-Output 'To use it yourself: Windows 11 Pro/Enterprise/Education, virtualization enabled in firmware, then'
  Write-Output 'Settings > System > Optional features > More Windows features > "Windows Sandbox" (restart needed).'
  return
}
if ($DryRun) { Write-Output 'Dry run: the sandbox was not started.'; return }
Write-Output 'Starting Windows Sandbox (only one Sandbox window can run; close any open one first) ...'
Write-Output ('After testing, the reports are in: ' + $results)
Start-Process -FilePath $wsb
