<#
  Runs INSIDE Windows Sandbox (started by the .wsb LogonCommand). Prepares a clean test of the portable
  Clade package and then leaves a PowerShell prompt open for the tester.

  What it does:
    1. verifies the ZIP against its .sha256;
    2. extracts it to the sandbox user's Desktop, in a folder whose name contains a space;
    3. prints the environment report and takes the "before" audit snapshot;
    4. writes C:\CladeQA\after.ps1 (the matching "after" snapshot + comparison, saved as UTF-8) and
       C:\CladeQA\run-after.cmd (runs it with a process-local ExecutionPolicy Bypass), then tells the
       tester what to do.

  It reads the read-only share C:\CladeQA\in and writes only inside the sandbox and to the dedicated
  results share C:\CladeQA\results. Nothing here touches the host except that results folder.
#>
$ErrorActionPreference = 'Stop'
$in = 'C:\CladeQA\in'
$qa = Join-Path $in 'qa'
$results = 'C:\CladeQA\results'
if (-not (Test-Path -LiteralPath $results)) { New-Item -ItemType Directory -Path $results -Force | Out-Null }
$work = Join-Path $env:USERPROFILE 'Desktop\Clade QA'
New-Item -ItemType Directory -Path $work -Force | Out-Null

$zip = Get-ChildItem -LiteralPath $in -Filter 'Clade-Launcher-Portable*.zip' | Select-Object -First 1
if (-not $zip) { throw "No Clade-Launcher-Portable*.zip in $in" }
Write-Host ('Package: ' + $zip.Name)

$sumFile = $zip.FullName + '.sha256'
$actual = (Get-FileHash -LiteralPath $zip.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
Write-Host ('SHA-256: ' + $actual)
if (Test-Path -LiteralPath $sumFile) {
  $expected = ((Get-Content -LiteralPath $sumFile -TotalCount 1) -split '\s+')[0].ToLowerInvariant()
  if ($expected -ne $actual) { throw "SHA-256 mismatch: expected $expected" }
  Write-Host 'Checksum file matches.'
} else { Write-Host 'No .sha256 next to the ZIP: checksum not verified.' }

$tar = Join-Path $env:SystemRoot 'System32\tar.exe'
& $tar -xf $zip.FullName -C $work
if ($LASTEXITCODE -ne 0) { throw 'Extraction failed.' }
$cladeRoot = Join-Path $work 'Clade-Launcher'
if (-not (Test-Path -LiteralPath (Join-Path $cladeRoot 'Clade.exe'))) { throw 'Clade.exe not found after extraction.' }
Write-Host ('Extracted to: ' + $cladeRoot)

& (Join-Path $qa 'environment-report.ps1') -CladeRoot $cladeRoot -OutFile (Join-Path $results 'environment-report.txt')
& (Join-Path $qa 'portable-audit.ps1') -Action Snapshot -Label before -OutDir $results -CladeRoot $cladeRoot

$after = @"
`$ErrorActionPreference = 'Stop'
& '$qa\portable-audit.ps1' -Action Snapshot -Label after -OutDir '$results' -CladeRoot '$cladeRoot'
`$b = (Get-ChildItem '$results' -Filter 'snapshot-before-*.json' | Sort-Object LastWriteTime | Select-Object -Last 1).FullName
`$a = (Get-ChildItem '$results' -Filter 'snapshot-after-*.json'  | Sort-Object LastWriteTime | Select-Object -Last 1).FullName
# Windows PowerShell 5.1: Tee-Object / Out-File default to UTF-16LE, so write the text explicitly as UTF-8.
`$out = (& '$qa\portable-audit.ps1' -Action Compare -Before `$b -After `$a *>&1 | Out-String)
`$code = `$LASTEXITCODE
Write-Host `$out
[System.IO.File]::WriteAllText('$results\audit-compare.txt', `$out, (New-Object System.Text.UTF8Encoding(`$true)))
Write-Host 'Comparison saved to $results\audit-compare.txt (host: the results folder printed by Start-QaSandbox.ps1).'
exit `$code
"@
Set-Content -LiteralPath 'C:\CladeQA\after.ps1' -Value $after -Encoding UTF8

# Guest-local runner: process-local ExecutionPolicy Bypass, so the Restricted default policy needs no change.
$cmd = '@echo off' + "`r`n" +
  'powershell.exe -NoProfile -ExecutionPolicy Bypass -File "C:\CladeQA\after.ps1"' + "`r`n" +
  'echo.' + "`r`n" + 'pause' + "`r`n"
Set-Content -LiteralPath 'C:\CladeQA\run-after.cmd' -Value $cmd -Encoding ASCII
Copy-Item -LiteralPath 'C:\CladeQA\run-after.cmd' -Destination (Join-Path $env:USERPROFILE 'Desktop\run-after.cmd') -Force

Write-Host ''
Write-Host '=========================================================================='
Write-Host ' CLADE QA - READY   (use only ONE Sandbox window; do not start a second one)'
Write-Host '=========================================================================='
Write-Host ' 1. Extracted Clade folder (run Clade.exe from here):'
Write-Host ('      ' + $cladeRoot)
Write-Host ('      & "' + (Join-Path $cladeRoot 'Clade.exe') + '"')
Write-Host ' 2. Run the rows of docs\QA-WINDOWS.md that apply, then CLOSE Clade.'
Write-Host ' 3. AFTER step - type exactly this (works under the Restricted execution policy):'
Write-Host '      C:\CladeQA\run-after.cmd'
Write-Host '    or double-click run-after.cmd on the Desktop.'
Write-Host ' 4. Results appear on the HOST in the results folder printed by Start-QaSandbox.ps1'
Write-Host ('    (guest path ' + $results + ').')
Write-Host '    Files: environment-report.txt, snapshot-before-*.json, snapshot-after-*.json, audit-compare.txt'
Write-Host ' Clipboard is disabled: type commands by hand. Do not change display settings;'
Write-Host ' drag the window edges by hand.'
Write-Host '=========================================================================='
