# Clade Launcher

Clade is an independent, minimal and portable launcher for Minecraft: Java Edition on Windows.

## Goals

- Simple and minimal desktop experience
- Portable installation — extract and run
- Support for multiple Minecraft versions
- Vanilla-first design
- Optional Fabric, Forge, NeoForge and Quilt profiles
- Multiple Microsoft / Minecraft accounts
- Isolated game instances
- No built-in mod, modpack, shader or resource-pack marketplace

## Authentication

Clade uses the Microsoft identity platform for account authentication.

Users authenticate through Microsoft's official sign-in flow. Clade does not collect or store Microsoft passwords.

Authentication credentials are stored locally in encrypted form and are never intentionally sent to third-party services.

## Minecraft Services

The launcher uses official Minecraft services to:

- verify Minecraft: Java Edition ownership
- retrieve the user's Java profile
- obtain the information required to launch Minecraft
- manage supported Java profile features such as skins and owned capes

## Privacy

Clade is designed as a local desktop application.

Launcher-controlled data is stored inside the portable launcher directory. The application does not require an account with any third-party mod distribution platform.

## Project status

Clade is currently under development.

Minecraft authentication and launching functionality are still being implemented.

## Disclaimer

Clade is an independent project and is not affiliated with, endorsed by, or sponsored by Mojang Studios or Microsoft.
