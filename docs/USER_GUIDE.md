# User guide

## First start

1. Start RustCraft.
2. Open **Accounts** and select **Add Microsoft account**. Your browser
   opens Microsoft's sign-in page. Sign in with the account that owns
   Minecraft: Java Edition, then return to the launcher.
3. Open **Instances** and select **New instance**.
4. Select **Play**.

You can skip step 2 if you only want to try the RustCraft engine (see
below). It needs no account.

## Instances

An instance is one game version with its own worlds, settings, resource
packs, mods, screenshots and logs. Instances are kept apart, so a mod in one
instance never affects another.

When you create an instance, you choose:

- **Version.** Search, or show snapshots too. Every Minecraft: Java Edition
  version from 1.8 is listed.
- **Mod loader.** None (vanilla), or Fabric with a loader version.
- **Engine** (only for 1.8.9): the official Java client, or the RustCraft
  engine.
- **Initial game options:** view distance and graphics quality. These seed
  the game's own options on the first start; later, change them in the
  game.

Select an instance's **Edit** button to change memory, Java, extra JVM
arguments, window size, or a server to join on launch. **Open folder** shows
the instance's files. **Mods** lists the instance's mods, says which ones
will not load and why, and turns mods on or off (see [MODS.md](MODS.md)).

## Playing

Select an instance and press **Play**. The first launch of a version
downloads its files; later launches reuse them and check that they are
intact. When the game window opens, the launcher window closes. When you
quit the game, the launcher comes back and shows how the session ended. If
the game crashed, the launcher links the log and the crash report.

### Offline

If Mojang's or Microsoft's services cannot be reached, the launcher offers
**Play offline (singleplayer only)**. This needs an installed version and an
account that passed an online check in the last 30 days. Servers do not work
in offline mode.

## The RustCraft engine

The RustCraft engine is RustCraft's own game client, written in Rust. It
plays Minecraft 1.8.9 worlds without Java. It is **experimental**: it plays
local worlds in creative mode, and many game features do not exist yet (see
[COMPATIBILITY.md](COMPATIBILITY.md#rustcraft-engine-native-no-java)).

To use it, create a 1.8.9 instance and select **RustCraft engine
(experimental)** as the engine. The first start creates a world called
"World" in the instance's `saves` folder.

### Controls

| Key | Action |
|---|---|
| Mouse | Look around (click the window first to capture the mouse) |
| W A S D | Move |
| Space | Jump; double-tap to start or stop flying; while flying, go up |
| Left Shift | Sneak (you do not fall off edges); while flying, go down |
| Left Ctrl | Sprint |
| Left mouse button | Break a block (hold to keep breaking) |
| Right mouse button | Place the selected block |
| Middle mouse button | Put the block you look at into the selected hotbar slot |
| 1–9, mouse wheel | Select a hotbar slot |
| F1 | Hide or show the interface |
| F2 | Save a screenshot to the instance's `screenshots` folder |
| F3 | Show or hide the debug screen (position, light, frame rate, GPU) |
| Esc | Pause menu: Back to game, Options, Save and quit |

The game also pauses when its window loses focus.

### Playing on a server

The engine can join servers for Minecraft 1.8 to 1.8.9. Open the instance's
**Edit** dialog and enter the server address (`host` or `host:port`). Press
**Play** to join it; clear the field to play the local world again.

- **Online-mode servers** (most public servers) need a signed-in account
  that owns the game. RustCraft signs you in to the server through Mojang's
  session service, as the official client does.
- **Offline-mode servers** accept any name and need no account.

On a server:

| Key | Action |
|---|---|
| T or Enter | Open the chat |
| / | Open the chat with a command started |
| Enter (in the chat) | Send |
| Esc | Close the chat, or open the game menu; **Disconnect** leaves the server |

In survival mode, hold the left mouse button to dig; the bar under the
crosshair shows the progress. The hotbar shows the blocks in your server
inventory. Health and food are shown above it. Other players, mobs and items
appear as outlined boxes (players with their names), because the engine has
no entity models yet; see
[COMPATIBILITY.md](COMPATIBILITY.md#rustcraft-engine-on-servers).

### Options

**Esc → Options** sets the render distance, field of view, mouse
sensitivity, VSync and a frame-rate limit (used when VSync is off). They are
saved in `rustcraft-options.toml` in the instance folder.

### Textures

The engine draws with its own built-in textures. For the original look,
either:

- start a 1.8.9 instance with the official client once while signed in, so
  that the launcher downloads the 1.8.9 client jar; the engine then reads the
  textures from that copy, or
- put a 1.8 resource pack (a folder or a `.zip`) into the instance's
  `resourcepacks` folder. Packs are used in name order, before the client
  jar.

### Saving

The engine saves changed chunks every 45 seconds, when they unload, and when
you quit. Before it first changes a region file in a session, it copies the
file to `rustcraft-backups/` in the world folder. Worlds are written in the
1.8 Anvil format that the official 1.8.9 client reads. Opening them in the
official client has not been tested yet, so keep a copy of worlds you care
about.

## Settings

- **Appearance:** reduce motion (turns off animations), and show snapshots
  in the version list by default.
- **Java:** the Java used when an instance has none of its own. Automatic
  (recommended) uses the Java runtime Mojang publishes for each version, or
  a matching installed Java.
- **Storage:** open the data folder, or clear temporary files.
- **Privacy:** what RustCraft sends over the network (only sign-in and game
  downloads; no telemetry).

## Help & diagnostics

This screen shows the RustCraft version, the platform, the data folder,
whether Microsoft sign-in is configured, the installed Java versions, recent
logs, crash reports and a table of what runs where. The logs contain no passwords or
tokens. See [TROUBLESHOOTING.md](TROUBLESHOOTING.md) for common problems.

## Where RustCraft keeps your files

| System | Folder |
|---|---|
| Windows | `%APPDATA%\RustCraft` |
| macOS | `~/Library/Application Support/RustCraft` |
| Linux | `~/.local/share/RustCraft` |

Inside it: `instances/` (one folder per instance), `versions/`,
`libraries/`, `assets/` and `runtimes/` (shared game files), `logs/`, and
`config.toml` and `accounts.toml`. Your sign-in is not in this folder; it is
in the system's credential store.
