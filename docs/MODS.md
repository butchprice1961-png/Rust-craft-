# Mods

## What works today

**Fabric mods with the official Java client.** Create an instance with
**Mod loader: Fabric** and choose a loader version. RustCraft installs the
loader profile from Fabric's metadata service (`meta.fabricmc.net`) and
checks that the profile belongs to the instance's game version. Put mod
`.jar` files into the instance's `mods` folder (**Open folder**), then press
Play. Fabric loads the mods inside the official game.

RustCraft reads the jars' metadata to check them (see below), but it never
changes or runs them. The mods run inside the game with the same rights as
the game, as they do with any other launcher, so install mods only from
sources you trust.

Status: tested with a hand-written Fabric profile through a mock server;
not yet tested against the real service (see
[COMPATIBILITY.md](COMPATIBILITY.md)).

## Checking mods

**Instances → Mods** lists every `.jar` in the instance's `mods` folder and
says, for each one, whether it will load:

- **Ready**: a Fabric mod whose requirements are all met.
- **Will not load**, with the reason:
  - the jar is made for Forge, NeoForge or Quilt, or has no mod metadata;
  - it needs another Minecraft version or a newer Fabric Loader;
  - it needs a mod that is missing, turned off, or the wrong version;
  - it declares that it breaks with an installed mod;
  - two jars contain the same mod;
  - the jar is damaged.
- **Warning**: for example, a server-only mod, which Fabric does not load in
  the game client.
- **Off**: the mod is turned off.

**Turn off** renames the jar to `<name>.jar.disabled`, and **Turn on**
renames it back. The game ignores `.disabled` files. RustCraft never changes
the contents of a jar.

The same check runs before each launch. If a mod will not load, the
launcher shows a warning, writes the full list to the launcher log, and
starts the game anyway.

How the check works: RustCraft reads `fabric.mod.json` (and the metadata of
the jars nested in it), `quilt.mod.json`, `META-INF/mods.toml`,
`META-INF/neoforge.mods.toml` and `mcmod.info` straight from the archive. It
never runs code from a jar and never unpacks one to disk. Metadata files
are limited to 1 MiB and nested jars to 64 MiB. Version requirements use
Fabric's rules (`>=`, `<`, `~`, `^`, `1.21.x`, `*`, and lists where any entry
may match). Requirements on Java are not checked, because the Java version
is chosen at launch. For weekly snapshots, Minecraft version requirements
are not checked either, because Fabric maps snapshots to versions with a
table that RustCraft does not have.

Limits: the check does not know about mods that a mod provides only at run
time, and it does not read `recommends` or `suggests`.

## What does not work yet

| Feature | Status |
|---|---|
| Forge, NeoForge, Quilt instances | Not supported |
| Mods in the RustCraft engine | Not supported. The engine has no mod interface yet, and it cannot run Java mods |
| Downloading or updating mods | Not implemented |
| Converting Java mods to the RustCraft engine | Not implemented |

## Plan

This is the intended design. Only step 1 exists so far, and RustCraft will
never say a mod works before it does.

1. **Scanner.** Done for Fabric instances; see "Checking mods" above.
2. **Compatibility report for the RustCraft engine.** It sorts every mod
   into exactly one of these classes:
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
