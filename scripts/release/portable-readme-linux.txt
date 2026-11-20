Clade Launcher {{VERSION}} (portable, Linux x64)
=================================================

This folder holds Clade Launcher in portable form. Everything stays in
this folder — accounts, settings, cached metadata and Minecraft instances
— without writing configuration or .minecraft folders into your home
directory.

Running Clade
-------------
1. Extract the whole Clade-Launcher folder anywhere you have write access.
2. Start ./Clade from your terminal or file manager (the executable bit is
   already set in the archive; if missing, run: chmod +x Clade).

Note for dual-boot setups: Clade (Linux) and Clade.exe (Windows) share
the same data/ and instances/ format when placed in the same folder.

System requirements
-------------------
- 64-bit Linux (x86_64) with an active Wayland or X11 desktop session.
- System GTK 3 and WebKitGTK 4.1 libraries (for example libwebkit2gtk-4.1-0
  on Debian/Ubuntu, webkit2gtk-4.1 on Arch/Fedora).
- Working OpenGL graphics drivers for Minecraft: Java Edition.

Moving or removing
------------------
Close Clade and Minecraft first, then copy or move the whole folder.
Delete the folder to remove Clade cleanly.

Licences
--------
Clade Launcher is licensed under the Apache License 2.0 (see LICENSE).
Third-party component licences are listed in THIRD-PARTY-NOTICES.txt.
