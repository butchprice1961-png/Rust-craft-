# Mods

## What works today

**Fabric mods with the official Java client.** Create an instance with
**Mod loader: Fabric** and choose a loader version. RustCraft installs the
loader profile from Fabric's metadata service (`meta.fabricmc.net`) and
checks that the profile belongs to the instance's game version. Put mod
`.jar` files into the instance's `mods` folder (**Open folder**), then press
Play. Fabric loads the mods inside the official game.

RustCraft does not read, change or check these jars. They run with the same
rights as the game, as they do with any other launcher, so install mods only
from sources you trust.

Status: tested with a hand-written Fabric profile through a mock server;
not yet tested against the real service (see
[COMPATIBILITY.md](COMPATIBILITY.md)).

## What does not work yet

| Feature | Status |
|---|---|
| Forge, NeoForge, Quilt instances | Not supported |
| Mods in the RustCraft engine | Not supported. The engine has no mod interface yet, and it cannot run Java mods |
| Mod manager in the launcher (list, enable, disable, update) | Not implemented |
| Mod scanning: reading `fabric.mod.json`, `quilt.mod.json`, `mods.toml`, `neoforge.mods.toml` and `mcmod.info`; dependency and version checks | Not implemented |
| Converting Java mods to the RustCraft engine | Not implemented |

## Plan

This is the intended design. None of it exists yet, and RustCraft will
never say a mod works before it does.

1. **Scanner.** Reads mod metadata from jars as zip files, without running
   any code. Every archive path is checked, and size limits stop hostile
   files. It reports the loader, the game versions, the dependencies and any
   conflicts. It never changes the original files.
2. **Compatibility report.** It sorts every mod into exactly one of these
   classes:
   - *Native:* a RustCraft engine mod (Rust or WebAssembly) that runs
     directly.
   - *Converted:* the mod contains only data, such as textures, models,
     sounds, languages, recipes, loot tables or tags, and RustCraft converts
     it to engine data.
   - *Java only:* the mod contains Java code, so it runs only in the
     official client.
   - *Unsupported:* the mod is broken, conflicts with another mod, or does
     not match the game version.

   The report gives the reason for each class. RustCraft does not translate
   Java code, so "converted" is never claimed for a mod that contains code.
3. **Approval.** Nothing is installed or enabled until you approve it in
   the launcher.
4. **Engine mod interface.** WebAssembly mods run in a sandbox with explicit
   permissions, for blocks, items, commands and events.
