# Architecture

RustCraft is a Cargo workspace. Every crate is in `crates/`.

```
rustcraft-launcher (bin: rustcraft)        rustcraft-client (bin: rustcraft-client)
  ├─ rustcraft-auth                          ├─ rustcraft-renderer
  ├─ rustcraft-assets                        │    └─ rustcraft-world
  ├─ rustcraft-mods                          ├─ rustcraft-protocol
  ├─ rustcraft-platform                      │    └─ rustcraft-world
  └─ rustcraft-core                          └─ rustcraft-world
                                                  └─ rustcraft-core
```

The launcher and the engine are separate programs. They share no code at
run time: the launcher starts `rustcraft-client` as a child process, as it
starts the official Java client.

## Crates

| Crate | What it does |
|---|---|
| `rustcraft-core` | Data folder layout (`DataDir`), safe joining of untrusted paths, atomic writes, TOML settings that survive damage, and an NBT reader and writer with limits |
| `rustcraft-auth` | Microsoft sign-in: OAuth with PKCE through the system browser and a loopback redirect, Xbox Live, XSTS, the Minecraft services login, ownership, profile and privileges; refresh tokens in the OS credential store (`keyring`); the account list |
| `rustcraft-assets` | Mojang metadata: the version manifest, version files (`inheritsFrom`, rules, natives, both argument formats), libraries, asset indexes and Java runtimes. A downloader with SHA-1 checks, resume, retries, concurrency and cancelling. It builds the game's command line |
| `rustcraft-mods` | Fabric Loader: lists loader versions and installs a loader profile. Reads mod metadata from jars (Fabric, Quilt, Forge, NeoForge) and checks mods against an instance with Fabric's version rules |
| `rustcraft-platform` | The single-instance lock, Java discovery and `java -version` parsing, and opening folders |
| `rustcraft-launcher` | The `eframe`/`egui` user interface, instances, settings, the launch pipeline and the game supervisor |
| `rustcraft-world` | 1.8 block data (ids, metadata, shapes, collision, textures), chunks and sections, sky and block light, ray casts, collisions, Anvil region files and `level.dat`, and the terrain generator |
| `rustcraft-renderer` | A `wgpu` renderer: textures from resource packs or built-in art in one texture array, section meshing, frustum culling, fog, the day–night sky, block outlines, and an off-screen render used by tests |
| `rustcraft-protocol` | Minecraft 1.8 networking (protocol 47): data types, framing, zlib compression, AES/CFB8 encryption, login (offline, or online through Mojang's session service), and the play packets the engine uses, including 1.8 chunk data. `examples/flat_server.rs` is a small test server |
| `rustcraft-client` | The RustCraft engine: window and input (`winit`), the 20 Hz game loop, player physics, block interaction, chunk streaming and saving, multiplayer (`remote.rs`), and the `egui` interface |

The specification also named `rustcraft-audio` and `rustcraft-tests`
crates. They do not exist yet, because there is no sound or separate test
suite to put in them. Tests live next to the code they test (`src/**` unit
tests and `crates/*/tests/`).

## Launching a game

1. **Prepare** (`rustcraft-launcher/src/game.rs`, on a Tokio runtime):
   - Official client: get a valid session (renew it if needed), install or
     verify the version's files, choose Java, and build the command line.
   - RustCraft engine: find `rustcraft-client` next to the launcher, collect
     resource packs, and write a JSON launch spec. For a server, first ask
     it for its version, and get a session if an account is signed in.
2. **Spawn.** The child's standard output and error go to
   `logs/<instance>-<time>.log`, with the token replaced by `<redacted>`.
   The engine receives its spec on standard input.
3. **Ready.** When the output contains a ready marker ("Setting user:",
   "LWJGL Version", "Backend library:" or the engine's `RUSTCRAFT_READY`),
   or after 40 seconds, the launcher closes its window.
4. **Supervise.** The launcher's main thread blocks on the child process
   handle. There is no polling, and the lock file stops a second launcher.
5. **Return.** When the game exits, the launcher opens its window again and
   shows the result: the exit status, the log, a crash report written
   during the session, and the engine's own reason if it gave one.

## The RustCraft engine

- **Game loop** (`client/src/app.rs`): `winit` events drive frames. Game
  logic runs at a fixed 20 ticks per second. Rendering interpolates between
  ticks.
- **World** (`world/src/world.rs`): a map of loaded chunks. Changing a block
  updates sky and block light, also across chunk borders.
- **Streaming** (`client/src/local.rs`): worker threads load chunks from
  region files, or generate and light new ones. The main thread adds a few
  finished chunks per frame. It writes changed chunks to disk every 45
  seconds, when they unload, and on exit.
- **Meshing** (`renderer/src/mesh.rs`): worker threads turn 16³ sections
  into vertex and index buffers, from a snapshot that includes a one-block
  border, so faces and light at section borders are correct.
- **Rendering** (`renderer/src/lib.rs`): one draw per visible section, with
  solid, cutout and translucent passes. Positions are relative to the
  camera, so precision holds far from the world origin.

## Multiplayer

A network thread (`client/src/remote.rs`) logs in, answers keep-alives
itself (so a busy or hidden window cannot time out) and decodes packets. The
game thread applies them within a time budget per frame, and sends movement
every tick, digging, placing and chat. Blocks the player changes are shown
at once; the server's block updates have the last word. Chunks from a
server keep the server's light.

## Design rules

- Untrusted input (metadata, archives, world files, resource packs) is
  checked at the boundary: paths, sizes and nesting.
- Secrets have a `Debug` form that hides them and never reach logs.
- Writes are atomic. Damaged settings files are kept aside, never
  overwritten.
- What is not supported fails with a message that says so.
