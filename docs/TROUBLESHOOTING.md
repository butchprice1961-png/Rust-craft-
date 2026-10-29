# Troubleshooting

Start with the logs. **Help & diagnostics** in the launcher opens them, and
they are also in the data folder:

| System | Data folder |
|---|---|
| Windows | `%APPDATA%\RustCraft` |
| macOS | `~/Library/Application Support/RustCraft` |
| Linux | `~/.local/share/RustCraft` (or `$XDG_DATA_HOME/RustCraft`) |

Set `RUSTCRAFT_HOME` to use another folder.

- `logs/launcher.log` is the launcher's log; `launcher.prev.log` is the log
  of the previous run.
- `logs/<instance>-<time>.log` is the output of one game session.
- `instances/<instance>/crash-reports/` holds the game's crash reports.
  For the RustCraft engine they are named `crash-<time>-client.txt`.

The logs contain no passwords or tokens, so you can share them when you
ask for help. They do contain your player name and folder paths.

## Sign-in

See the table of error messages in [AUTHENTICATION.md](AUTHENTICATION.md#error-messages).

**The browser did not open.** Copy the link that the launcher shows into
any browser on the same computer. The sign-in returns to `localhost`, so it
must finish on the same computer.

**The browser says it cannot reach `localhost` after sign-in.** A firewall
or a proxy setting may block local connections. Allow connections to
`127.0.0.1`, or turn off "use a proxy for local addresses" in the browser.

**"the saved sign-in has expired"**. Microsoft revoked the saved sign-in,
for example after a password change. Sign in again.

**Linux: "credential store unavailable"** in the log, and you must sign in
on every start. No Secret Service is running. Install and unlock GNOME
Keyring or KWallet. Without one, RustCraft keeps the sign-in only until the
launcher closes.

## Downloads and launching

**"network error fetching …"** RustCraft could not reach Mojang's servers.
Check the connection, a proxy or a firewall. Versions that are already
installed can still start: when only the network is the problem, the
launcher offers "Play offline (singleplayer only)" for accounts verified
online in the last 30 days.

**"… expected SHA-1 …, got …"** A downloaded file did not match the hash
that Mojang publishes, even after retries. This usually means a proxy or
antivirus program changed the file. Try again on another network.
RustCraft does not use files that fail the check.

**"… is missing and has no download URL"** The version uses a library that
no server offers for download. This happens with version profiles that
another tool installed, such as the Forge installer. RustCraft does not
support Forge or NeoForge.

**"<version> needs Java N"** Mojang's Java runtime for this version could
not be downloaded, and no installed Java fits. Reconnect and try again, or
install Java N (Java 8 for 1.8–1.16, 16 for 1.17, 17 for 1.18–1.20.4, 21
for 1.20.5 and later) and select it in **Settings** or in the instance's
options.

**"The Java at … could not be run."** The Java path in the settings is
wrong. Clear it to use automatic selection.

**The game closes immediately ("failed to start").** Open the session log
from the message. Common causes:

- The memory setting is larger than the free memory. Lower the maximum
  memory in the instance's options.
- Extra JVM arguments are not valid for this Java version. Remove them.
- A mod does not fit the game or loader version. Remove recently added mods
  from the instance's `mods` folder.

**The game crashed.** The launcher links the crash report. With mods, the
report usually names the mod that caused the crash.

**"RustCraft is already running".** Only one launcher can run at a time. If
a game is open, the launcher comes back when you quit the game. If no
RustCraft window is visible, check the task manager for a `rustcraft`
process.

## RustCraft engine

**"\"World\" is a survival world…"** The engine plays only creative worlds
so far. Open survival worlds with the official client (a 1.8.9 instance that
uses the official Java client).

**"The RustCraft engine (rustcraft-client) is missing from this
installation."** `rustcraft-client` must be in the same folder as the
launcher program. Install RustCraft again.

**"this server is in online mode and needs a signed-in Minecraft account"**
Sign in in the launcher with the account that owns the game, then join
again.

**"Mojang refused the session; sign in again in the launcher"** The saved
sign-in has expired or was revoked. Sign out and sign in again.

**"Could not join …: connection failed …"** Check the address and port. If
the server's address works in the official client but not here, the server
may rely on a DNS SRV record, which RustCraft does not look up yet; use the
server's host name and port directly.

**"Disconnected: …"** The server closed the connection and gave this
reason, for example a kick, a ban, a whitelist or a wrong game version. The
engine only joins servers for Minecraft 1.8 to 1.8.9.

**"no compatible GPU found"** The engine needs Vulkan, Metal, DirectX 12 or
OpenGL ES 3 support. Update your graphics drivers. On Linux, install your
distribution's Mesa Vulkan drivers (for example `mesa-vulkan-drivers`).

**Very low frame rate.** Check the GPU line in the F3 screen. If it names
`llvmpipe` or another software renderer, the GPU driver is not in use. Lower
the render distance in the pause menu (Esc → Options). See
[BENCHMARKS.md](BENCHMARKS.md) to measure.

**Textures look different from Minecraft.** RustCraft ships its own simple
textures. To use the original ones, sign in and start a 1.8.9 instance with
the official client once, so that the launcher downloads the 1.8.9 client
jar. Or put a 1.8 resource pack into the instance's `resourcepacks` folder.

**A world looks wrong after the engine saved it.** Each session's first
change to a region file is backed up in the world's `rustcraft-backups`
folder. To restore a region, copy its backup back into `region/` and remove
the time stamp from the name. Please report the problem with the world.

## Linux: the launcher does not start

**"install libxkbcommon-x11"** Run `sudo apt install libxkbcommon-x11-0`
(Debian and Ubuntu) or the equivalent for your distribution.

**"a system graphics or input library is missing"** Install the X11 or
Wayland libraries and Mesa: on Debian and Ubuntu, `libxkbcommon-x11-0`,
`libgl1-mesa-dri` and `mesa-vulkan-drivers`.

**AppImage: "FUSE" errors.** Install your distribution's FUSE package, or
run the AppImage with `--appimage-extract-and-run`.

## Reset

- **One instance:** in **Instances**, use Delete. Worlds are deleted too, so
  copy `instances/<name>/saves` first if you want to keep them.
- **Temporary files:** **Settings → Clear temporary files** deletes the
  download cache and unpacked native libraries. RustCraft downloads or
  unpacks them again when it needs them.
- **Settings:** delete `config.toml` in the data folder.
- **Accounts:** sign out of each account in **Accounts**. This also deletes
  the saved sign-in from the credential store.
