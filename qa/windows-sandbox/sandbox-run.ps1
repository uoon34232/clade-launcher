<#
  Runs INSIDE Windows Sandbox (started by the .wsb LogonCommand). Prepares a clean test of the portable
  Clade package and then leaves a PowerShell prompt open for the tester.

  What it does:
    1. verifies the ZIP against its .sha256;
    2. extracts it to the sandbox user's Desktop, in a folder whose name contains a space;
    3. prints the environment report and takes the "before" audit snapshot;
    4. writes C:\CladeQA\after.ps1 (the matching "after" snapshot + comparison) and tells the tester what to do.

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
& '$qa\portable-audit.ps1' -Action Compare -Before `$b -After `$a | Tee-Object -FilePath '$results\audit-compare.txt'
"@
Set-Content -LiteralPath 'C:\CladeQA\after.ps1' -Value $after -Encoding UTF8

Write-Host ''
Write-Host '--------------------------------------------------------------------------'
Write-Host 'Ready. Start Clade yourself:'
Write-Host ('  & "' + (Join-Path $cladeRoot 'Clade.exe') + '"')
Write-Host 'Run the rows of docs\QA-WINDOWS.md that apply. When finished, close Clade and run:'
Write-Host '  C:\CladeQA\after.ps1'
Write-Host ('Reports are kept in ' + $results + ' (the only folder shared back to the host).')
Write-Host 'Do not change display settings or resize the window to extreme sizes; drag its edges by hand.'
Write-Host '--------------------------------------------------------------------------'
