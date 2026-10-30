# Compatibility

This page says what RustCraft supports and how each claim was checked. It
uses four words:

- **Verified**: tested end to end in the real environment.
- **Tested**: covered by automated tests that use mock services or
  hand-written data in the official format. It has not run against the real
  services or files.
- **Experimental**: works in the cases described, with known gaps.
- **Not supported**: does not exist yet. RustCraft says so instead of
  pretending.

No line on this page is "verified" against Mojang's or Microsoft's live
services yet. The environment RustCraft was built in could not reach them
(see "Not yet verified" at the end).

## Launcher

| Feature | Status | How it was checked |
|---|---|---|
| Window, screens, dark theme, reduced motion | Verified on Linux | Every screen checked from screenshots on a virtual display; CI screenshots the Play screen after each lifecycle test |
| Single instance (a second launch shows a message and exits) | Tested | Unit test of the lock |
| Close while playing, reopen on game exit | Verified on Linux | `scripts/lifecycle-smoke.sh` in CI: a stand-in Java game, and the real native engine |
| Crash and early-exit reporting | Verified on Linux | Lifecycle test in CI; early exit checked by hand with a world the native engine refuses, which shows the engine's reason |
| Windows and macOS | Tested | Unit tests pass on `windows-2022` and `macos-14` in CI. The window and packages have not been run there yet; the release workflow does that and has not run yet |

## Official game versions (Java Edition, run with Java)

RustCraft lists every version in Mojang's version manifest from 1.8 to the
newest release and snapshot, in the manifest's own order. Versions older
than 1.8 are not offered.

| Feature | Status | How it was checked |
|---|---|---|
| Version list, search, snapshots toggle | Tested | Hand-written manifest in Mojang's format |
| Version files: client jar, libraries, natives, assets, log configuration | Tested | Installs from hand-written 1.8.9 and 1.21.4 version files through a mock server; every file is checked against its SHA-1 |
| Old (1.8–1.12, `minecraftArguments`) and new (1.13+, `arguments` with rules) command lines | Tested | Unit tests for both formats, including OS rules, natives and Quick Play |
| Mojang Java runtimes (`jre-legacy`, `java-runtime-*`) | Tested | Installs a hand-written runtime manifest through a mock server |
| Installed Java fallback | Tested | Unit tests for `java -version` parsing and for the selection rules; discovery runs against the JDK on the CI machines |
| Offline relaunch from the cache | Tested | Install, stop the server, launch again |
| Resume after an interrupted download, retries, cancel | Tested | Mock server that fails, sends corrupt data, and ignores `Range` |
| Launching real game files, 1.8 to latest | Not yet verified | Needs network access to Mojang's servers |

Java requirement: each version's file names the Java it needs (Java 8 for
1.8–1.16, Java 16 for 1.17, Java 17 for 1.18–1.20.4, Java 21 for 1.20.5 and
later). RustCraft downloads Mojang's matching runtime. If that is not
possible, it uses an installed Java of that major version or newer. Java 8
is the exception: versions that need it get exactly Java 8, because older
game versions break on newer Java.

## Mods

| Loader | Status |
|---|---|
| Fabric Loader | Tested: installs the loader profile from Fabric's metadata service and runs it with Java. Checked with a hand-written profile through a mock server, not yet with the real service |
| Quilt | Not supported |
| Forge, NeoForge | Not supported |
| Mod checks (loader, game version, dependencies, duplicates), turning mods on and off | Tested: unit tests with generated jars, including oversized and damaged archives |
| Native Rust/WASM mods, mod conversion | Not supported yet; see [MODS.md](MODS.md) |

## RustCraft engine (native, no Java)

The RustCraft engine is RustCraft's own client, written in Rust. It does not
run Mojang's code. Status: **experimental**.

| Feature | Status |
|---|---|
| Game version | 1.8.9 only |
| Worlds | Local worlds in the 1.8 Anvil format: new worlds, and existing creative worlds. Tested with worlds RustCraft wrote; not yet with worlds saved by vanilla 1.8.9. Survival, adventure and spectator worlds are refused with a message, because survival rules do not exist yet |
| Saving | Changed chunks every 45 seconds, when unloaded, and on exit. Each region file gets one backup per session in `rustcraft-backups/` before the first overwrite. Data RustCraft does not model (entities, tile entities, unknown fields) is kept as it was |
| New-world terrain | RustCraft's own generator: ocean, beach, plains, desert, mountains, forest, birch forest, taiga and ice plains, with oak, birch and spruce trees, flowers, grass and cacti. It does not reproduce vanilla terrain for a seed |
| Blocks | All 1.8.9 block ids (0–197) with metadata. Vanilla geometry for cubes, plants, crops, slabs, straight stairs, fences, walls, panes, gates, doors, trapdoors, torches, ladders, vines, rails, liquids, snow layers and flat blocks. Chests, signs, beds, banners, skulls, corner stairs and some others are drawn as simple boxes |
| Lighting | Sky and block light, recomputed when blocks change; the vanilla brightness curve; smooth day and night sky |
| Movement | Vanilla 1.8 walking, sprinting, sneaking (with edge stop), jumping, step-up, swimming and creative flight. Unit tests compare speeds and jump height with vanilla values |
| Interaction | Breaking and placing blocks, a 9-slot hotbar and pick block. Placement sets the orientation of torches, logs, slabs, stairs, pumpkins, furnaces, chests and dispensers. Doors and beds are placed as a single half |
| Textures | Built-in original art. If you add a 1.8 resource pack to the instance's `resourcepacks` folder, it is used. If the launcher has downloaded the official 1.8.9 client jar (from a signed-in launch of a 1.8.9 Java instance), its textures are used, read from your own copy |
| Settings | Render distance 2–32, field of view, mouse sensitivity, VSync and a frame-rate limit, in the pause menu |
| Not yet available | Survival mode in local worlds, mobs and other entities, inventory and crafting screens, fluid flow, redstone, sound, the Nether and the End |

## Multiplayer

| Feature | Status |
|---|---|
| Joining servers with the official client | Tested: the instance's server field starts the game with `--server`/`--port` (1.8–1.19) or Quick Play (1.20+). The launcher blocks the join if Minecraft services report that multiplayer is disabled for the account |
| RustCraft engine multiplayer (protocol 47) | Experimental; see below |

### RustCraft engine on servers

The engine speaks protocol 47, the network protocol of Minecraft 1.8 to
1.8.9. It joins a server set in the instance's options.

How it was tested: unit tests for its data types and main packets, the
NIST test vector for the AES/CFB8 stream cipher, Minecraft's documented
server-hash examples, and an in-process online-mode server that performs
the full login with RSA, encryption, compression and a mock session
service. A small test server (`cargo run -p rustcraft-protocol --example
flat_server`) checks the client end to end in CI. **It has not been tested
against a real server implementation (vanilla, Spigot, Paper or Cuberite)
yet**: building and running one was not allowed in the environment where
RustCraft was built.

| Feature | Status |
|---|---|
| Offline-mode servers | Join with the player name; no account needed |
| Online-mode servers | Join with the signed-in account through Mojang's session service, as the official client does. Without a signed-in account, the engine says the server needs one |
| Compression and encryption | Yes |
| The world | Chunk data and chunk bulks, block and multi-block changes, time of day, unloading |
| Movement | Sent every tick like the 1.8 client; the server's corrections are applied |
| Creative mode | Instant breaking; placing any block from the hotbar (the server is told which item it is) |
| Survival and adventure mode | Digging with 1.8's timing for the held tool; placing blocks from the server's hotbar; health and food shown as numbers; death screen with respawn |
| Chat | Receiving (including the common translation keys) and sending messages and commands (T, /) |
| Server version | Before joining, the launcher asks the server for its status, like the server list does. A server that reports another protocol is refused with its version named, instead of a kick for an outdated client. Multi-version servers that accept 1.8 clients answer with protocol 47 and are joined |
| Disconnects | The server's reason is shown |
| Other players, mobs and objects | Shown as outlined boxes of their real size (white: players, red: hostile mobs, green: other mobs, yellow: items and other objects), with players' name tags. There are no entity models or skins yet |
| Not yet available | Entity models, skins and animations; attacking entities; the inventory screen and containers; signs' text; weather; sounds; Nether and End skies; server addresses that rely on SRV records; walking through blocks in spectator mode; a server list screen |

## Platforms

| Platform | Build | Run |
|---|---|---|
| Linux x86_64 | CI | Verified (Ubuntu 22.04 in CI, Ubuntu 24.04 locally). Needs X11 or Wayland, Vulkan or OpenGL drivers, and `libxkbcommon-x11` on X11 |
| Windows x86_64 | CI | Tested (unit tests only) |
| macOS arm64 and x86_64 | CI | Tested (unit tests only) |

## Not yet verified

These hosts were blocked in the build environment, so the matching features
have only been tested with mocks:

- `piston-meta.mojang.com`, `launchermeta.mojang.com`, `libraries.minecraft.net`, `resources.download.minecraft.net`: game files
- `user.auth.xboxlive.com`, `xsts.auth.xboxlive.com`, `api.minecraftservices.com`: sign-in. (`login.microsoftonline.com` was reachable, but the rest of the chain was not.)
- `meta.fabricmc.net`: Fabric

To verify them, run the launcher on a normal connection with an approved
client ID (see [AUTHENTICATION.md](AUTHENTICATION.md)). Then install and
start 1.8.9, 1.12.2, 1.16.5, 1.20.4 and the newest release, and a Fabric
instance.
