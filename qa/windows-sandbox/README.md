# Windows Sandbox harness for Clade

A disposable, clean Windows 11 desktop for validating the portable Clade ZIP. Everything in it is thrown
away when the sandbox window is closed, so it cannot damage the host and always starts clean.

This folder only holds configuration and the script that runs **inside** the sandbox. The host-side
launcher is [`scripts/qa/Start-QaSandbox.ps1`](../../scripts/qa/Start-QaSandbox.ps1). Nothing here changes
Clade, the host's Windows settings, display, GPU or optional features.

## Is it available on this PC?

Windows Sandbox is an optional Windows feature (Windows 10/11 Pro, Enterprise or Education, with
virtualization enabled in firmware). **These scripts never enable it.** `Start-QaSandbox.ps1` only checks
that `%SystemRoot%\System32\WindowsSandbox.exe` exists; if it does not, it stages everything, prints how to
enable the feature yourself (Settings > System > Optional features > More Windows features > Windows
Sandbox, then restart) and stops.

The development machine this was written on (Windows 11 Pro 26300) does **not** have the feature installed
(no `WindowsSandbox.exe`), so the launch path below was validated only up to the dry run.

## Configurations

| File | Network | Virtual GPU | Use it for |
| --- | --- | --- | --- |
| `clade-offline.wsb` | off | off (software rendering) | first launch, relocation, settings, damage and recovery, offline prepared launch (a profile must already be prepared in the package under test) |
| `clade-online.wsb` | on | off | preparation and download rows (Java, Minecraft, loaders) |
| `clade-online-gpu.wsb` | on | on | rows that must open a Minecraft window |

Common to all three: audio, video, printer and clipboard redirection **disabled**; 8 GB memory; exactly two
mapped folders and no others:

* the staging folder, **read-only**, as `C:\CladeQA\in` (the ZIP, its `.sha256`, the QA scripts);
* one empty results folder, **writable**, as `C:\CladeQA\results` (reports come back through it).

Not shared, ever: the repository, the user profile, the real `%APPDATA%\.minecraft`, any real Clade folder
or data, the host's Documents. The templates contain the placeholders `%CLADE_QA_STAGING%` and
`%CLADE_QA_RESULTS%` instead of any developer path; `Start-QaSandbox.ps1` writes a resolved copy next to
the staged files (outside the repository). Windows 11 23H2 and later also expand environment variables in
`HostFolder`, so you can set those two variables and open a template directly. Relative paths are not
supported by Windows Sandbox.

Why the virtual GPU is off by default: Microsoft notes that vGPU increases the sandbox's attack surface,
and with it off the sandbox renders in software (WARP). Software rendering cannot run Minecraft (OpenGL),
so use `online-gpu` only for the rows that start the game.

## Use

```powershell
# 1. Build or obtain Clade-Launcher-Portable.zip (docs/RELEASE.md). Never use a real account.
# 2. From the repository, stage and start (nothing is enabled or installed on the host):
scripts\qa\Start-QaSandbox.ps1 -Zip C:\path\to\Clade-Launcher-Portable.zip -Mode offline
#    add -DryRun to stage and resolve the .wsb without starting anything.
```

Inside the sandbox, `sandbox-run.ps1` (started by the LogonCommand) verifies the checksum, extracts the
package to `Desktop\Clade QA\` (a path with a space), prints the environment report, takes the **before**
audit snapshot and prints what to do next. Start `Clade.exe` by hand, run the rows from
[`docs/QA-WINDOWS.md`](../../docs/QA-WINDOWS.md), close Clade and run `C:\CladeQA\after.ps1` for the
**after** snapshot and the comparison. Reports (`environment-report.txt`, the two snapshots,
`audit-compare.txt`) land in `C:\CladeQA\results`, which is the host's `results` folder.

The sandbox user (`WDAGUtilityAccount`) is an administrator inside the sandbox; Clade itself is `asInvoker`
and still must not ask for elevation (that is a test row).

## Command line (optional)

Windows 11 24H2 and later ship `wsb.exe` (`wsb start --config "<Configuration>...</Configuration>"`,
`wsb list`, `wsb exec`, `wsb share`, `wsb stop`, ...). It could drive fully scripted runs, but it is not used
here: `wsb exec` cannot return a command's output (no process I/O) and needs an active session, so the
LogonCommand + results-folder design is simpler and works on every Windows Sandbox version that supports
`.wsb` files (Windows 10 build 18342 and Windows 11).

## Limits

* The sandbox window size is not configurable; do not try to force one. Check the 960x600 layout by
  dragging the window edges by hand.
* Display scaling inside the sandbox follows the host. Test 100 / 125 / 150% by changing scaling yourself
  in Settings before starting the sandbox, or on a real machine; this harness never changes it.
* No GPU means no game window; with `online-gpu` the host GPU is used through Microsoft's virtualization.
* A sandbox has a Microsoft Edge / WebView2 runtime of its own; the bundled-runtime check is done by looking
  at which `msedgewebview2.exe` the Clade process tree uses (see the runbook), not by the absence of a
  system runtime.
* Closing the sandbox deletes everything not written to the results folder.
