<#
.SYNOPSIS
  Builds Clade-Launcher-Portable.zip (and its .sha256): a self-contained folder for any 64-bit Windows PC
  or USB drive.

.DESCRIPTION
  1. Checks that the version is the same in every place it is written (and matches -Tag when given).
  2. Builds the production Clade.exe the Tauri way (pnpm tauri build --no-bundle --target x86_64-pc-windows-msvc),
     with the C runtime linked statically and developer paths remapped out of the binary.
  3. Checks the pinned WebView2 Fixed Version Runtime CAB against the SHA-256 in
     scripts/release/webview2-runtime.json, extracts it, and requires a valid Microsoft signature and the
     pinned version.
  4. Generates THIRD-PARTY-NOTICES.txt from the dependency lock files.
  5. Stages ONLY the allowed files in a clean folder, writes a deterministic ZIP and its SHA-256, and
     validates the finished ZIP against the package allowlist.

  The distributable binary is never built with plain `cargo build --release`: the Tauri build is what
  embeds the interface and uses the production protocol.

.PARAMETER WebView2Cab
  The x64 Fixed Version CAB obtained directly from Microsoft. Its SHA-256 must equal the pin in
  scripts/release/webview2-runtime.json, which a person must fill in (see docs/RELEASE.md). There is no
  way around the pin for a release package.

.PARAMETER Tag
  The release tag (for example v0.1.0-preview). When given, it must match the version and the changelog.

.PARAMETER SkipBuild
  Reuse an already built src-tauri\target\x86_64-pc-windows-msvc\release\clade-launcher.exe (the CI job
  builds it in an earlier step with the same flags).

.PARAMETER WithoutWebView2
  Development only. Produces Clade-Launcher-Portable-NO-WEBVIEW2.zip, which depends on the PC's own
  WebView2 and can never pass the release validation.
#>
[CmdletBinding(DefaultParameterSetName = 'Release')]
param(
  [Parameter(ParameterSetName = 'Release', Mandatory = $true)] [string] $WebView2Cab,
  [Parameter(ParameterSetName = 'Dev', Mandatory = $true)] [switch] $WithoutWebView2,
  [string] $Tag,
  [string] $OutDir,
  [switch] $SkipBuild
)

$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path
$target = 'x86_64-pc-windows-msvc'
$outDir = [System.IO.Path]::GetFullPath($(if ($OutDir) { $OutDir } else { Join-Path $repo 'dist-portable' }))
$rel = Join-Path $repo 'scripts\release'
function Run-Node { & node @args; if ($LASTEXITCODE) { throw "node $($args -join ' ') failed" } }
function ConvertTo-TomlString([string] $value) {
  return '"' + $value.Replace('\', '\\').Replace('"', '\"') + '"'
}

# ---- 1. one version everywhere
$checkArgs = @((Join-Path $rel 'check-versions.mjs'), '--root', $repo)
if ($Tag) { $checkArgs += @('--tag', $Tag) }
Run-Node @checkArgs
$version = (Get-Content -LiteralPath (Join-Path $repo 'package.json') -Raw | ConvertFrom-Json).version

# ---- 1b. the pinned WebView2 runtime is checked FIRST, so a missing pin or a wrong CAB fails in seconds, not after a build
if (-not $WithoutWebView2) {
  Run-Node (Join-Path $rel 'webview2.mjs') status                  # says exactly what is not configured yet
  $cab = (Resolve-Path -LiteralPath $WebView2Cab).Path
  Run-Node (Join-Path $rel 'webview2.mjs') verify --cab $cab       # SHA-256 against the pin, before anything is extracted
}

# ---- 2. the production build
$exe = Join-Path $repo "src-tauri\target\$target\release\clade-launcher.exe"
# Strings that must not end up inside the shipped binary: this machine's profile and source folders, in
# long and 8.3 form. Rust and C paths are remapped at build time; the validation proves it worked.
$shortPath = { param($p) try { (New-Object -ComObject Scripting.FileSystemObject).GetFolder($p).ShortPath } catch { $null } }
$cargoHome = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $env:USERPROFILE '.cargo' }
$rustupHome = if ($env:RUSTUP_HOME) { $env:RUSTUP_HOME } else { Join-Path $env:USERPROFILE '.rustup' }
$forbid = @($env:USERPROFILE, $repo, $cargoHome, $rustupHome) + @($env:USERPROFILE, $repo, $cargoHome, $rustupHome | ForEach-Object { if ($_ -and (Test-Path -LiteralPath $_)) { & $shortPath $_ } }) |
  Where-Object { $_ } | Sort-Object -Unique
if (-not $SkipBuild) {
  $cargoCfg = Join-Path ([System.IO.Path]::GetTempPath()) ('clade-cargo-' + [guid]::NewGuid().ToString('N') + '.toml')
  Push-Location -LiteralPath $repo
  try {
    # Static CRT and path remapping only for the final target artifacts via a TOML array in Cargo --config
    # (never space-split RUSTFLAGS), so paths in $repo or $env:USERPROFILE containing spaces remap cleanly
    # while host build scripts and proc-macros stay on the normal toolchain path.
    $rustFlags = New-Object System.Collections.Generic.List[string]
    $rustFlags.Add('-C')
    $rustFlags.Add('target-feature=+crt-static')
    foreach ($pair in @(
        @($env:USERPROFILE, '/user'),
        @($cargoHome, '/cargo'),
        @($rustupHome, '/rustup'),
        @($repo, '/clade')
      )) {
      $from = $pair[0]
      $to = $pair[1]
      if (-not $from) { continue }
      $rustFlags.Add("--remap-path-prefix=$from=$to")
      if (Test-Path -LiteralPath $from) {
        $short = & $shortPath $from
        if ($short -and ($short -ne $from)) { $rustFlags.Add("--remap-path-prefix=$short=$to") }
      }
    }
    $tomlItems = ($rustFlags | ForEach-Object { ConvertTo-TomlString $_ }) -join ', '
    $tomlText = "[target.$target]`nrustflags = [$tomlItems]`n"
    [System.IO.File]::WriteAllText($cargoCfg, $tomlText, (New-Object System.Text.UTF8Encoding($false)))

    # The C code that is compiled into the binary (the TLS library) records its source paths too; MSVC's
    # /d1trimfile removes a folder prefix from them. Both the long and the 8.3 forms are covered.
    $env:CFLAGS = ($forbid | Where-Object { $_ -notmatch '[ "]' } | ForEach-Object { '/d1trimfile:' + $_ + '' }) -join ' '
    if ($Tag) { $env:CLADE_RELEASE_TAG = $Tag } else { Remove-Item -LiteralPath Env:CLADE_RELEASE_TAG -ErrorAction SilentlyContinue }
    pnpm install --frozen-lockfile
    if ($LASTEXITCODE) { throw 'pnpm install failed' }
    pnpm tauri build --no-bundle --target $target '--' --locked --config $cargoCfg
    if ($LASTEXITCODE) { throw 'tauri build failed' }
  } finally {
    Remove-Item -LiteralPath Env:CFLAGS -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath Env:CLADE_RELEASE_TAG -ErrorAction SilentlyContinue
    if (Test-Path -LiteralPath $cargoCfg) { Remove-Item -LiteralPath $cargoCfg -Force -ErrorAction SilentlyContinue }
    Pop-Location
  }
}
if (-not (Test-Path -LiteralPath $exe)) { throw "Release binary not found: $exe" }

# ---- 3. the WebView2 Fixed Version Runtime
$wvDir = $null
if (-not $WithoutWebView2) {
  $cfg = Get-Content -LiteralPath (Join-Path $rel 'webview2-runtime.json') -Raw | ConvertFrom-Json
  $work = Join-Path ([System.IO.Path]::GetTempPath()) ('clade-wv2-' + [guid]::NewGuid().ToString('N'))
  New-Item -ItemType Directory -Path $work | Out-Null
  try {
    & (Join-Path $env:windir 'System32\expand.exe') -F:* $cab $work | Out-Null
    if ($LASTEXITCODE) { throw "expand.exe failed on $cab" }
    $found = Get-ChildItem -LiteralPath $work -Recurse -Filter msedgewebview2.exe | Select-Object -First 1
    if (-not $found) { throw "msedgewebview2.exe not found inside $cab" }
    $wvSource = $found.Directory.FullName
    $sig = Get-AuthenticodeSignature -LiteralPath (Join-Path $wvSource 'msedgewebview2.exe')
    if ($sig.Status -ne 'Valid' -or $sig.SignerCertificate.Subject -notmatch 'O=Microsoft Corporation') {
      throw "msedgewebview2.exe is not validly signed by Microsoft Corporation (status: $($sig.Status))."
    }
    $wvVersion = (Get-Item -LiteralPath (Join-Path $wvSource 'msedgewebview2.exe')).VersionInfo.ProductVersion
    if ($wvVersion -ne $cfg.version) { throw "WebView2 runtime is $wvVersion but the pinned version is $($cfg.version)." }
    Write-Host "WebView2 Fixed Version ${wvVersion}: SHA-256 matches the pin, Authenticode signature valid"
    $wvDir = Join-Path $outDir 'webview2-extracted'
    if (Test-Path -LiteralPath $wvDir) { Remove-Item -LiteralPath $wvDir -Recurse -Force }
    New-Item -ItemType Directory -Path $outDir -Force | Out-Null
    Copy-Item -LiteralPath $wvSource -Destination $wvDir -Recurse
  } finally { Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue }
}

# ---- 4. notices, 5. package + validation
New-Item -ItemType Directory -Path $outDir -Force | Out-Null
$notices = Join-Path $outDir 'THIRD-PARTY-NOTICES.txt'
Push-Location -LiteralPath $repo
try { Run-Node (Join-Path $rel 'generate-notices.mjs') --out $notices --strict } finally { Pop-Location }

$pkgArgs = @((Join-Path $rel 'package.mjs'), '--exe', $exe, '--license', (Join-Path $repo 'LICENSE'), '--notices', $notices,
  '--webview2-license', (Join-Path $PSScriptRoot 'licenses\WebView2-Fixed-Version-License.txt'), '--version', $version, '--out', $outDir)
if ($WithoutWebView2) { $pkgArgs += '--no-webview2' } else { $pkgArgs += @('--webview2', $wvDir) }
Run-Node @pkgArgs

$zipName = if ($WithoutWebView2) { 'Clade-Launcher-Portable-NO-WEBVIEW2.zip' } else { 'Clade-Launcher-Portable.zip' }
$zip = Join-Path $outDir $zipName
$valArgs = @((Join-Path $rel 'validate-package.mjs'), $zip, '--version', $version, '--csp-from', (Join-Path $repo 'src-tauri\tauri.conf.json'))
foreach ($f in $forbid) { $valArgs += @('--forbid-string', $f) }
if ($WithoutWebView2) { $valArgs += '--allow-missing-webview2' }
Run-Node @valArgs

if ($wvDir -and (Test-Path -LiteralPath $wvDir)) { Remove-Item -LiteralPath $wvDir -Recurse -Force }   # large and already inside the ZIP
$hash = (Get-FileHash -LiteralPath $zip -Algorithm SHA256).Hash.ToLower()
$expected = ((Get-Content -LiteralPath "$zip.sha256" -Raw) -split '\s+')[0]
if ($hash -ne $expected) { throw "SHA-256 file ($expected) does not match the ZIP ($hash)." }
Write-Host ""
Write-Host "Created $zip ($([math]::Round((Get-Item -LiteralPath $zip).Length / 1MB, 1)) MB)"
Write-Host "SHA-256 $hash"
if ($WithoutWebView2) { Write-Warning 'This build has NO bundled WebView2 runtime and is for development only; it fails the release validation.' }
