<#
.SYNOPSIS
  Builds Clade-Launcher-Portable.zip: a self-contained folder that runs from any Windows x64 PC or USB drive.

.DESCRIPTION
  1. Builds the frontend and a release Clade.exe with the C runtime linked statically
     (no Visual C++ redistributable needed on the target PC).
  2. Assembles Clade-Launcher/ with the standard portable layout.
  3. Bundles a WebView2 *Fixed Version* runtime into Clade-Launcher/runtimes/webview2/ so the target PC
     needs no WebView2 installation. This is required; there is no silent fallback to an installer.
  4. Zips the folder and writes a SHA-256 file next to it.

.PARAMETER WebView2Dir
  Directory holding the extracted Fixed Version runtime (it must contain msedgewebview2.exe).
  Get the x64 "Fixed Version" CAB from https://developer.microsoft.com/microsoft-edge/webview2/
  and extract it with:  expand.exe -F:* <file>.cab <target-dir>   (the CAB contains one versioned folder;
  point -WebView2Dir at that folder). The runtime is NOT updated automatically: re-package to ship fixes.

.PARAMETER WithoutWebView2
  Development only. Produces a zip that depends on the target PC already having WebView2. The resulting
  zip is named Clade-Launcher-Portable-NO-WEBVIEW2.zip so it cannot be mistaken for a release.
#>
[CmdletBinding(DefaultParameterSetName = 'Release')]
param(
  [Parameter(ParameterSetName = 'Release')] [string] $WebView2Dir,
  [Parameter(ParameterSetName = 'Release')] [string] $WebView2Cab,
  [Parameter(ParameterSetName = 'Dev', Mandatory = $true)] [switch] $WithoutWebView2,
  [string] $OutDir
)

$ErrorActionPreference = 'Stop'
$repo = Resolve-Path (Join-Path $PSScriptRoot '..')
$target = 'x86_64-pc-windows-msvc'
if (-not $OutDir) { $OutDir = Join-Path $repo 'dist-portable' }
$outDir = [System.IO.Path]::GetFullPath($OutDir)

if (-not $WithoutWebView2) {
  if ($WebView2Cab) {
    # Microsoft.WebView2.FixedVersionRuntime.<ver>.x64.cab -> extract with the built-in expand.exe.
    $cab = (Resolve-Path $WebView2Cab).Path
    $WebView2Dir = Join-Path ([System.IO.Path]::GetTempPath()) ('wv2-' + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $WebView2Dir | Out-Null
    & (Join-Path $env:windir 'System32\expand.exe') -F:* $cab $WebView2Dir | Out-Null
    if ($LASTEXITCODE) { throw "expand.exe failed on $cab" }
    $found = Get-ChildItem $WebView2Dir -Recurse -Filter msedgewebview2.exe | Select-Object -First 1
    if (-not $found) { throw "msedgewebview2.exe not found inside $cab" }
    $WebView2Dir = $found.Directory.FullName
  }
  if (-not $WebView2Dir) { throw 'Pass -WebView2Cab <file.cab> or -WebView2Dir <folder> (or -WithoutWebView2 for development only).' }
  $WebView2Dir = (Resolve-Path $WebView2Dir).Path
  $wv = Join-Path $WebView2Dir 'msedgewebview2.exe'
  if (-not (Test-Path $wv)) { throw "msedgewebview2.exe not found in '$WebView2Dir'." }
  # Never ship a runtime that is not genuinely signed by Microsoft.
  $sig = Get-AuthenticodeSignature $wv
  if ($sig.Status -ne 'Valid' -or $sig.SignerCertificate.Subject -notmatch 'O=Microsoft Corporation') {
    throw "msedgewebview2.exe is not validly signed by Microsoft Corporation (status: $($sig.Status))."
  }
  Write-Host "WebView2 Fixed Version runtime $((Get-Item $wv).VersionInfo.ProductVersion), Authenticode signature valid"
}

Push-Location $repo
try {
  # Static CRT only for the final target artifacts. Passing --target keeps build scripts and
  # proc-macros on the normal host toolchain path, so incremental builds stay fast.
  $env:CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS = '-C target-feature=+crt-static'
  pnpm install --frozen-lockfile
  if ($LASTEXITCODE) { throw 'pnpm install failed' }
  pnpm tauri build --no-bundle --target $target   # --no-bundle: never produce an installer
  if ($LASTEXITCODE) { throw 'tauri build failed' }
} finally {
  Remove-Item Env:CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS -ErrorAction SilentlyContinue
  Pop-Location
}

$exe = Join-Path $repo "src-tauri\target\$target\release\clade-launcher.exe"
if (-not (Test-Path $exe)) { throw "Release binary not found: $exe" }

$stage = Join-Path $outDir 'Clade-Launcher'
if (Test-Path $stage) { Remove-Item $stage -Recurse -Force }
New-Item -ItemType Directory -Path $stage | Out-Null
Copy-Item $exe (Join-Path $stage 'Clade.exe')

# Standard layout (the app also creates anything missing on first start).
foreach ($d in 'data','instances','runtimes','runtimes\java','minecraft','libraries','assets','cache','cache\tmp','cache\downloads','cache\updates','logs') {
  New-Item -ItemType Directory -Path (Join-Path $stage $d) -Force | Out-Null
}

if (-not $WithoutWebView2) {
  Copy-Item $WebView2Dir (Join-Path $stage 'runtimes\webview2') -Recurse
  # The runtime's license requires passing its terms on to end users.
  $lic = Join-Path $stage 'licenses'
  New-Item -ItemType Directory -Path $lic -Force | Out-Null
  Copy-Item (Join-Path $PSScriptRoot 'licenses\*.txt') $lic
}

@"
Clade Launcher (portable)
===================
Run Clade.exe. Nothing needs to be installed and no administrator rights are required.

Everything the launcher stores (accounts, instances, Minecraft files, Java runtimes, logs) lives in this
folder. Copy or move the WHOLE folder to take it to another PC. Do not copy Clade.exe alone.

 - Use an NTFS or exFAT drive. FAT32 cannot store files larger than 4 GB.
 - Close Clade before unplugging the drive.
 - Keep the folder path short (for example E:\Clade-Launcher) when possible.
 - Your accounts are protected by a master passphrase; Microsoft may still ask you to sign in again.
"@ | Set-Content -Path (Join-Path $stage 'README.txt') -Encoding UTF8

$zipName = if ($WithoutWebView2) { 'Clade-Launcher-Portable-NO-WEBVIEW2.zip' } else { 'Clade-Launcher-Portable.zip' }
$zip = Join-Path $outDir $zipName
if (Test-Path $zip) { Remove-Item $zip -Force }
Add-Type -AssemblyName System.IO.Compression.FileSystem
[System.IO.Compression.ZipFile]::CreateFromDirectory($stage, $zip, [System.IO.Compression.CompressionLevel]::Optimal, $true)

$hash = (Get-FileHash $zip -Algorithm SHA256).Hash.ToLower()
"$hash  $zipName" | Set-Content -Path "$zip.sha256" -Encoding ASCII
Write-Host ""
Write-Host "Created $zip ($([math]::Round((Get-Item $zip).Length / 1MB, 1)) MB)"
Write-Host "SHA-256 $hash"
if ($WithoutWebView2) { Write-Warning 'This build has NO bundled WebView2 runtime and is for development only.' }
