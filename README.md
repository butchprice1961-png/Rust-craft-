# RustCraft

RustCraft is a launcher and game client for Minecraft: Java Edition, written
in Rust. It signs in with your Microsoft account, installs and runs the
official game from version 1.8 to the latest snapshot, and includes an
experimental native engine that plays 1.8.9 worlds without Java.

RustCraft is not an official Minecraft product. It is not approved by or
associated with Mojang or Microsoft. It ships no Mojang code or assets: game
files are downloaded from Mojang only after you sign in with an account that
owns the game.

## Status

This is an early release (0.1.0, unreleased). The table says what works and
how it was checked. Details are in [docs/COMPATIBILITY.md](docs/COMPATIBILITY.md).

| Area | State |
|---|---|
| Launcher UI, instances, settings, logs, crash reports | Working on Linux, where CI renders it on a virtual display. Unit tests also pass on Windows and macOS, but the UI has not been run there yet |
| Launcher lifecycle (close while playing, reopen on exit, single instance) | Working on Linux; tested end to end in CI with a stand-in game and with the native engine |
| Microsoft sign-in (browser + PKCE, Xbox Live, XSTS, ownership, profile) | Implemented; tested against a mock of every service. Not yet tested against the live services |
| Official game downloads and launch, 1.8 to latest | Implemented; tested with hand-written 1.8.9 and 1.21.4 metadata in Mojang's format and a mock download server. Not yet tested with real game files |
| Fabric Loader instances | Implemented; tested with a hand-written Fabric profile. Not yet tested live |
| Forge, NeoForge, Quilt | Not supported |
| Native engine (RustCraft engine) | Experimental: 1.8.9 local worlds in creative mode. No survival in local worlds, no mobs, inventory screen or sound yet |
| Native engine multiplayer (1.8 servers, protocol 47) | Experimental: joins offline- and online-mode servers, shows the world, chat, digging and placing. Other players and mobs are not shown yet. Tested against RustCraft's own test servers only, not yet against a real server implementation |
| Mod checks for Fabric instances (loader, versions, dependencies), turning mods on and off | Implemented; tested with generated jars |
| Mod conversion for the native engine | Not implemented yet |
| Windows installer, macOS dmg, Linux AppImage | Release workflow written; it runs on a tag or manual dispatch and has not run yet |

## Install

Releases are not published yet. To build from source, see
[docs/BUILDING.md](docs/BUILDING.md). In short, with Rust 1.97:

```sh
cargo build --release -p rustcraft-launcher -p rustcraft-client
RUSTCRAFT_MSA_CLIENT_ID=<your Azure application id> target/release/rustcraft
```

Sign-in needs an Azure application that Mojang has approved for Minecraft
services. [docs/AUTHENTICATION.md](docs/AUTHENTICATION.md) explains how to
get one. Without it, the launcher still runs and the native engine still
plays local worlds, but official game versions cannot be launched.

## Documentation

- [User guide](docs/USER_GUIDE.md): the launcher, instances, the native engine and its controls
- [Compatibility](docs/COMPATIBILITY.md): what is supported, and how each part was tested
- [Troubleshooting](docs/TROUBLESHOOTING.md): error messages and what to do about them
- [Authentication](docs/AUTHENTICATION.md): how sign-in works and how to configure it
- [Security](docs/SECURITY.md): how credentials, downloads and files are protected
- [Mods](docs/MODS.md): Fabric support today and the plan for the rest
- [Benchmarks](docs/BENCHMARKS.md): how performance is measured, and the measurements so far
- [Architecture](docs/ARCHITECTURE.md): how the code is organised
- [Building](docs/BUILDING.md): toolchain, system packages, tests and packaging

## License

MIT, see [LICENSE](LICENSE). The Inter font is under the SIL Open Font
License (`crates/rustcraft-launcher/assets/fonts/Inter-LICENSE.txt`).
Minecraft is a trademark of Mojang Synergies AB.
