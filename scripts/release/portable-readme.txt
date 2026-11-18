Clade Launcher {{VERSION}} (portable)
=====================================

Clade is a portable launcher for Minecraft: Java Edition on Windows.

Requirements
 - Windows 10 (64-bit) or Windows 11 (64-bit) (x64).
 - Nothing needs to be installed and no administrator rights are needed.

To start
 1. Extract the whole ZIP to a normal folder. Do not run Clade.exe from inside the ZIP.
 2. Run Clade.exe.

Preview build, not code-signed
 - This is an early Preview and it is not code-signed.
 - Windows SmartScreen may warn about Clade.exe.
 - Windows 11 Smart App Control, when it runs in enforcement mode, may block Clade.exe completely and
   may offer no "Run anyway" option. Please do not disable or bypass Smart App Control, Defender or any
   other Windows security feature to run Clade.

Your data
 - Everything Clade stores (accounts, profiles, game files, Java runtimes, settings, logs) is kept in
   this folder, next to Clade.exe. Clade creates its folders here the first time it starts.
 - Keep the whole folder together. You can move or copy the folder to another place, drive or PC;
   copy the folder, not Clade.exe on its own.
 - Close Clade before unplugging a drive that holds it.

Tips
 - Use an NTFS or exFAT drive. FAT32 cannot hold files larger than 4 GB. On Windows 10, an NTFS folder
   uses Clade's bundled WebView2 directly, whereas an exFAT drive uses the PC's installed WebView2
   runtime (so prefer NTFS if a Windows 10 PC may not have WebView2 installed).
 - On Windows 10, where Windows 11's Mica backdrop is unavailable, Clade uses its opaque warm paper
   window look.
 - For the best compatibility with older Minecraft and Java versions, keep the folder in a reasonably
   short path that uses only English letters, digits and plain punctuation (for example D:\Clade-Launcher).
 - Microsoft may ask you to sign in again, on any computer.
 - Forgot the passphrase that protects your sign-ins? It cannot be recovered. Choose "Forgot passphrase?"
   on the unlock screen, then "Start fresh". The old encrypted store is kept in data\retired-vaults, you
   sign in again with a new passphrase, and your profiles, worlds, playtime and settings stay as they are.

Reporting a bug
 - https://github.com/uoon34232/clade-launcher/issues/new/choose
 - Say which Clade and Windows version you use, what happened and how to reproduce it. Settings >
   "Export diagnostics" saves a redacted ZIP under logs\diagnostics next to Clade.exe; look through
   it before you attach it.
 - Never post passwords, Microsoft sign-in codes, tokens, callback addresses or vault files.
 - Security problems: see SECURITY.md in the project repository, not a public issue.

Licences
 - LICENSE (Apache License 2.0), THIRD-PARTY-NOTICES.txt and the licenses folder.

Project: https://github.com/uoon34232/clade-launcher
