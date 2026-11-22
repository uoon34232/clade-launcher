# Windows QA runbook

How to validate the portable Clade ZIP on Windows 10 x64 and Windows 11 x64 without risking the machine you test on. It is a
checklist for people, backed by small read-only scripts. It adds no product behaviour.

Related: [RELEASE.md](RELEASE.md) (how the ZIP is built), [`qa/windows-sandbox/`](../qa/windows-sandbox/README.md)
(clean disposable desktop), [`scripts/qa/`](../scripts/qa/) (the helpers).

## 1. Ground rules

1. **Test on a clean machine.** Prefer Windows Sandbox (offline / online / online-gpu configurations) or a
   VM with a snapshot. Your daily PC has a system WebView2, maybe Java, a real `.minecraft` and real
   accounts, which weakens every "clean machine" claim.
2. **Credentials are typed by a person, never scripted.** Real Microsoft sign-in works (Mojang approved the
   application ID) and is tested by hand: the person enters the passphrase, the password and any MFA code
   themselves; a script or an assistant never types, reads or captures them, and no screenshot is taken while a
   sign-in page or its callback address is visible. A development fixture identity only proves the launch
   pipeline, not authentication, and a fixture launch must be labelled as one.
3. **Never change the host's display, GPU, resolution, DPI or driver as part of a test**, and never script
   native window resizing. Resize by dragging the window edge by hand. (An earlier automation bug that
   requested a 976x65535 window crashed the Desktop Window Manager and the GPU driver had to be reset by
   rebooting. There is deliberately no window-geometry helper in `scripts/qa/`; if one is ever written it
   must reject widths outside 640-3840, heights outside 480-2160 and absurd coordinates, print the requested
   geometry first and offer a dry-run mode.)
4. **Use disposable copies** of the Clade folder for every damage test, and disposable data only.
5. **Do not fix anything while testing.** Reproduce, record the smallest evidence, classify, move on.

## 2. Tools

| Tool                                | What it is for                                                                      | Touches                                                                        |
| ----------------------------------- | ----------------------------------------------------------------------------------- | ------------------------------------------------------------------------------ |
| `scripts/qa/Start-QaSandbox.ps1`    | stage a ZIP and start Windows Sandbox                                               | a temp staging folder; starts the sandbox if installed; never enables features |
| `scripts/qa/environment-report.ps1` | Windows build, DPI, GPU/driver status, WebView2, Java, disk, file system, elevation | nothing (read-only; optional `-OutFile`)                                       |
| `scripts/qa/portable-audit.ps1`     | before/after snapshot of the places a portable app must not write, and a comparison | nothing but its own JSON files in `-OutDir`                                    |
| `scripts/smoke-test-package.ps1`    | the automated package smoke test (checksum, allowlist, start, bundled runtime)      | an extraction folder under `%TEMP%`                                            |

Inside Windows Sandbox the same before/after steps are wrapped for you: after testing, close Clade and run
`C:\CladeQA\run-after.cmd` (process-local `-ExecutionPolicy Bypass`; the Restricted policy is never changed).
Use only one Sandbox window at a time and keep the clipboard disabled; see
[`qa/windows-sandbox/README.md`](../qa/windows-sandbox/README.md).

Typical run on any machine:

```powershell
.\scripts\qa\environment-report.ps1 -CladeRoot 'D:\Clade Launcher\Clade-Launcher' -OutFile .\env.txt
.\scripts\qa\portable-audit.ps1 -Action Snapshot -Label before -OutDir .\qa-out -CladeRoot 'D:\Clade Launcher\Clade-Launcher'
# run Clade and the matrix rows below, then close Clade
.\scripts\qa\portable-audit.ps1 -Action Snapshot -Label after  -OutDir .\qa-out -CladeRoot 'D:\Clade Launcher\Clade-Launcher'
.\scripts\qa\portable-audit.ps1 -Action Compare -Before .\qa-out\snapshot-before-*.json -After .\qa-out\snapshot-after-*.json
```

The compare step exits 0 only when nothing changed outside the Clade folder in the watched locations
(`%APPDATA%\.minecraft`, AppData folders named like Clade, Documents, top-level AppData/ProgramData/profile
folders, Clade registry keys, Run/RunOnce keys). Windows-owned artifacts (MuiCache and similar entries it
writes for any exe it has seen) are listed as INFO, not as failures. So is the Microsoft Edge auto-launch Run value
(`MicrosoftEdgeAutoLaunch_<32 hex>` pointing at the Program Files `msedge.exe`) that Edge creates itself after the
browser sign-in Clade opens for Microsoft OAuth; any other Run value stays EXTERNAL.

Optional, third-party-free extra: Microsoft's Sysinternals **Process Monitor** shows every file and registry
operation of `Clade.exe`. It loads a kernel driver and needs administrator rights, so use it only inside a
sandbox or VM when a snapshot comparison shows a surprise and you need to know who wrote it.

## 3. Recording results

Each row ends in exactly one of: `PASS`, `FAIL`, `BLOCKED` (say by what), `NOT TESTED` (say why). Keep:
the environment report, the audit comparison, the Clade log (`logs/clade.log`), and a screenshot only where a
row asks for one. Copy the table below into the test notes and fill the last column.

## 4. Matrix

"Evidence" is the smallest thing that proves the row.

### A. Package, location, WebView2

| ID  | Scenario                | Setup                                                                                     | Expected                                                                                                                                                                                                                                                                                     | Evidence                                                                                                                                          | Result |
| --- | ----------------------- | ----------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------- | ------ |
| A1  | Clean first launch      | Fresh sandbox/VM; extract the ZIP; run `Clade.exe`                                        | Window opens, onboarding shows once, no UAC prompt, folders `data`, `instances`, `runtimes`, `minecraft`, `libraries`, `assets`, `cache`, `logs` appear beside `Clade.exe`                                                                                                                   | screenshot of onboarding; folder listing; no elevation prompt; `logs/clade.log` says `webview=Bundled`                                            |        |
| A2  | Relocation              | Prepared folder moved/copied to another path (and back)                                   | Everything works; profiles stay Ready; settings kept                                                                                                                                                                                                                                         | profile list after the move; `logs/clade.log`; text files contain no absolute path (`findstr /s /i "C:\\" data\*.json instances\*\instance.json`) |        |
| A3  | Spaces and Unicode path | Run from `...\Clade Launcher\` and from a Cyrillic folder name                            | Starts; a modern profile prepares and launches                                                                                                                                                                                                                                               | launch screenshot; game log header shows `<clade>` paths                                                                                          |        |
| A4  | Another drive           | Folder on a second NTFS (and, separately, an exFAT) drive                                 | Works; FAT32 refused/limited only by file size                                                                                                                                                                                                                                               | environment report file system line; launch result                                                                                                |        |
| A5  | Bundled WebView2        | Any run on a machine with and, if you can, without a system runtime                       | Every `msedgewebview2.exe` of the Clade tree runs from `runtimes\webview2`; profile under `data\webview2`                                                                                                                                                                                    | Task Manager / `Get-CimInstance Win32_Process` paths; `webview2` line in `logs/clade.log`                                                         |        |
| A6  | Windows 10 and UNC      | Windows 10 x64 VM on NTFS and on exFAT (with Evergreen WebView2); separate UNC path check | Windows 10 NTFS grants `S-1-15-2-1` and `S-1-15-2-2` RX ACLs on `runtimes\webview2` without elevation and logs `webview=Bundled`; Windows 10 exFAT falls back to `webview=System` when Evergreen WebView2 is present; UNC paths log a warning, fall back to `System`, and remain unsupported | `icacls runtimes\webview2` output on Win10 NTFS; `webview=` lines in `logs/clade.log`                                                             |        |
| A7  | No admin rights         | Standard user (not elevated)                                                              | Clade never requests elevation                                                                                                                                                                                                                                                               | environment report "Elevated (administrator): False"; no UAC prompt                                                                               |        |

### B. Java and Minecraft

| ID  | Scenario                                   | Setup                                                                   | Expected                                                                                                                                                         | Evidence                                                            | Result |
| --- | ------------------------------------------ | ----------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------- | ------ |
| B1  | No Java installed                          | Machine without Java (environment report: none on PATH, no `JAVA_HOME`) | Clade needs none                                                                                                                                                 | environment report Java section                                     |        |
| B2  | Managed Java from empty                    | Empty `runtimes\java`; prepare a modern profile (online)                | Mojang runtime downloaded into `runtimes\java\...`, profile Ready                                                                                                | progress dialog; `runtimes\java` listing; log                       |        |
| B3  | Modern vanilla                             | Latest release profile                                                  | Game window opens on the managed Java                                                                                                                            | game window screenshot; `logs\games\<profile>\*.log` header         |        |
| B4  | Legacy Java 8                              | `1.12.2` profile                                                        | Opens on `jre-legacy`                                                                                                                                            | window screenshot; java path in the session log header              |        |
| B5  | Fabric / B6 Forge / B7 NeoForge / B8 Quilt | One 1.21.x profile per loader                                           | Prepare then start the JVM on the managed Java; Forge/NeoForge installers run inside the sandboxed staging                                                       | profile Ready; game starts; no change outside the root (audit)      |        |
| B9  | Offline prepared launch                    | Prepared profile; network off (offline sandbox)                         | Launches without network (needs a launch-ready account state)                                                                                                    | launch result; log has no network error                             |        |
| B10 | Real sign-in and launch                    | A Microsoft account that owns Minecraft, signed in by hand (see rule 2) | Sign-in completes, the account and its skin avatar appear, the account survives a restart without a new sign-in, and a prepared profile reaches the title screen | account card; `logs\clade.log` (no tokens); title-screen screenshot |        |

### C. Settings, diagnostics, recovery

| ID  | Scenario                 | Setup                                                                                              | Expected                                                                                                  | Evidence                                                   | Result |
| --- | ------------------------ | -------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------- | ------ |
| C1  | Settings persistence     | Change language, theme (Follow Windows, Light, Dark), sound, transparency, reduced motion; restart | All kept in `data\settings.json`; Light and Dark ("Graphite Paper") palettes persist cleanly              | file contents; UI after restart                            |        |
| C2  | Malformed settings       | Empty / truncated / wrong-typed `settings.json` (copy of the folder)                               | Starts; damaged file kept as `settings.json.corrupt`; defaults; profiles untouched                        | listing; log WARN line                                     |        |
| C3  | Diagnostics export       | Settings > Export diagnostics after a game run                                                     | ZIP in `logs\diagnostics`; only the allowlisted entries; no tokens, accounts, vault, worlds, user paths   | extract the ZIP and search it for fake secrets you planted |        |
| C4  | Damaged runtime          | Delete / corrupt (same size) a managed Java file in a copy                                         | Play offers repair; "Verify files" fixes same-size damage                                                 | dialog screenshot; file hash restored                      |        |
| C5  | Interrupted download     | Kill Clade during preparation                                                                      | `.part` files remain; next Set up resumes; no `.part` after                                               | file listing before/after                                  |        |
| C6  | Corrupt profile manifest | Truncate one `instance.json` in a copy                                                             | Omitted from playable list, UI warning shown, files untouched, others fine, WARN in log                   | warning screenshot; log line; other profiles listed        |        |
| C7  | Read-only folder         | Deny write on `data\` (ACL on the disposable copy only)                                            | Localized "can't write to its folder" dialog, no crash                                                    | dialog screenshot                                          |        |
| C8  | Low disk                 | Disposable small volume / quota (never the system drive)                                           | Preparation stops early with a clear message                                                              | dialog; nothing partial left                               |        |
| C9  | No external writes       | The `portable-audit.ps1` comparison around any run                                                 | EXTERNAL list empty                                                                                       | the comparison output                                      |        |
| C10 | In-app portable update   | Settings > Check for updates (when newer release exists) > Update > Restart to update              | Downloads and verifies `.sha256`/ZIP, applies `--apply-update` swap, and preserves profiles/data/settings | updated `Clade.exe` version; `data\` & `instances\` intact |        |

### D. Window and UI

| ID  | Scenario                   | Setup                                                                                                                                  | Expected                                                                                                                                                                                                                                                                           | Evidence                                          | Result |
| --- | -------------------------- | -------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------- | ------ |
| D1  | Default and minimum size   | Drag the window edge to the smallest size by hand                                                                                      | 960x600 inner minimum; Settings fits without a scrollbar or clipping; Profile menu actions stay visible with 6+ profiles                                                                                                                                                           | screenshots at minimum size (EN and RU)           |        |
| D2  | 100% / 125% / 150% scaling | Set scaling in Windows Settings **yourself** before the test (never by script)                                                         | Layout intact, text crisp, no clipping                                                                                                                                                                                                                                             | screenshot per scale; environment report DPI line |        |
| D3  | Window controls            | Drag the title strip; double-click it; minimize; close (idle and with a framed game)                                                   | Drags when unmaximized; minimize/close work; when idle, double-click does not maximize and maximize/full screen are unavailable; while a game is framed in `embed` mode, Maximize/Restore button and header double-click toggle maximize/restore (full screen remains unavailable) | notes                                             |        |
| D4  | Second instance            | Start `Clade.exe` again (also while minimized and while hidden during a game)                                                          | Existing window comes forward; no second launcher state                                                                                                                                                                                                                            | one `Clade.exe` process; window restored          |        |
| D5  | Transparency effects       | Settings: Follow Windows, On, Off (in Light and Dark / "Graphite Paper"); toggle Windows "Transparency effects" and OS Light/Dark mode | Windows 11 (22H2+) uses Light or dark Mica (`DWMWA_USE_IMMERSIVE_DARK_MODE`) and follows live; Off is fully opaque; Windows 10 stays on the opaque Light/Dark paper look; Reduced motion independent                                                                               | screenshots Light/Dark On/Off; log                |        |
| D6  | Language                   | Switch English / Russian                                                                                                               | Whole UI switches, kept after restart                                                                                                                                                                                                                                              | screenshots                                       |        |

### E. Process and crash behaviour

| ID  | Scenario                             | Setup                                                        | Expected                                                                                             | Evidence                                                                         | Result |
| --- | ------------------------------------ | ------------------------------------------------------------ | ---------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------- | ------ |
| E1  | Duplicate launch                     | Click Play twice / second launch of the same profile         | One game only                                                                                        | one `javaw.exe` (or `java.exe` fallback)                                         |        |
| E2  | Hide / show policies                 | "When Minecraft starts: Hide", "When it closes: Stay hidden" | Window hides on launch; a clean exit quits Clade; a crash brings the window back with an explanation | window states; log                                                               |        |
| E3  | Crash classification                 | Stop the game process abruptly                               | "Minecraft stopped" dialog with Open logs                                                            | screenshot; session log footer                                                   |        |
| E4  | Launcher killed while Minecraft runs | Force-kill `Clade.exe` during a game                         | Game keeps running (it ends only when you close it); output keeps being written                      | `javaw.exe` (or `java.exe` fallback) alive for 5+ minutes; no broken-pipe errors |        |
| E5  | Recovery                             | Start Clade again after E4                                   | Unfinished playtime session recovered; stale output file emptied                                     | log line `recovered unfinished playtime sessions`; `cache\game-out` empty        |        |

## 5. Known environment pitfalls

- A machine where the display driver has fallen back to Microsoft Basic Display Adapter (Device Manager
  code 43) cannot run OpenGL games; the environment report shows the driver status. Re-test after a normal
  reboot; do not try to repair the driver from test scripts.
- Windows may write MuiCache / app-compat entries for every `Clade.exe` path it has seen; the audit lists
  them as INFO, they are not Clade's.
- **Smart App Control blocks an unsigned `Clade.exe`.** Windows Sandbox runs it in enforcement mode, so an
  unsigned development or release package fails to start there ("An Application Control policy has blocked
  this file"; CodeIntegrity events 3033, 3077 and 3118). Do not disable Smart App Control, change the policy or
  work around it from a test script. Run the tests that need a started Clade on a machine or VM whose Smart
  App Control state allows it, and record the Sandbox rows as `BLOCKED (Smart App Control: unsigned build)`
  until Clade is code-signed (not implemented yet). Smart App Control also checks the DLLs a game loads, which
  signing Clade does not change.
- Windows Sandbox runs one instance at a time: close the old window (its contents are discarded) before
  starting another run, and copy results out first.
- Antivirus or SmartScreen may block an unsigned `Clade.exe`; that is a documented limitation.

## 6. References

Official Microsoft documentation used to design the harness:

- Windows Sandbox configuration files (`.wsb`): <https://learn.microsoft.com/windows/security/application-security/application-isolation/windows-sandbox/windows-sandbox-configure-using-wsb-file>
- Windows Sandbox command line (`wsb.exe`, Windows 11 24H2+): <https://learn.microsoft.com/windows/security/application-security/application-isolation/windows-sandbox/windows-sandbox-cli>
- WebView2 distribution, Fixed Version runtime (expand with `expand`, Windows 10 ACL requirement from v120, not from UNC paths): <https://learn.microsoft.com/microsoft-edge/webview2/concepts/distribution>
- High-DPI desktop application development (awareness modes, what to test): <https://learn.microsoft.com/windows/win32/hidpi/high-dpi-desktop-application-development-on-windows>
- Sysinternals Process Monitor: <https://learn.microsoft.com/sysinternals/downloads/procmon>
