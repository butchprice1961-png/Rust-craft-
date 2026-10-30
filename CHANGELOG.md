# Changelog

## 0.1.0 (unreleased)
- Launcher with Microsoft account sign-in (browser, PKCE), account switching and sign-out.
- Instances for every Minecraft: Java Edition version from 1.8, each with its own worlds, settings and mods.
- Verified, resumable downloads of game files and Mojang's Java runtimes, with offline relaunch from cache.
- Fabric Loader instances; Forge and NeoForge are not supported yet.
- The launcher closes while you play and reopens when the game exits, with crash reports and logs one click away.
- Mod checks for Fabric instances: each mod's loader, Minecraft and Fabric Loader versions and dependencies, with a warning before launch; mods can be turned on and off.
- The RustCraft engine joins 1.8 servers (offline and online mode): world, movement, digging, placing and chat; other players and mobs show as outlined boxes with name tags.
- Experimental RustCraft engine for 1.8.9 instances: local worlds in creative mode, rendered natively with wgpu.
- Not verified yet against Mojang's live services; see docs/COMPATIBILITY.md.
