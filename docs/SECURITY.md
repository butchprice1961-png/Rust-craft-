# Security

This page explains how RustCraft protects your account, your computer and
your files, and where the limits are.

## Reporting a vulnerability

Please report security problems privately, through a GitHub security
advisory on this repository. Do not open a public issue for them. Include
the RustCraft version, your operating system and the steps to reproduce.

## Your Microsoft account

[AUTHENTICATION.md](AUTHENTICATION.md) describes the sign-in flow. In short:

- You type your password only into Microsoft's own page, in your own browser.
  RustCraft never sees it.
- Sign-in uses PKCE and a random `state`. The local redirect server listens
  on `127.0.0.1` only, ignores requests with a wrong `state`, and closes
  after one sign-in.
- RustCraft has no client secret. The client ID is public by design.
- The renewable token is kept only in the operating system's credential
  store. RustCraft never writes tokens to its own files, and it never asks
  you to paste a token anywhere.
- Tokens never appear in RustCraft's logs or in its interface.

### One limit: the official client's command line

The official Minecraft client reads its access token from a command-line
argument (`--accessToken`), as Mojang's version files define the launch.
While the game runs, programs running under your user account can read that
argument from the process list. On Linux, by default, other users of the
same computer can read it too. The token stays valid for about 24 hours.

A small Java start-up class that receives the token through a pipe would
remove this exposure. RustCraft does not have one yet.

The RustCraft engine does not have this problem: the launcher passes its
launch data through a pipe, never on the command line.

## Downloads

- Every connection uses HTTPS. The download client refuses plain-HTTP
  addresses, including redirects to them. Sign-in requests go to fixed
  HTTPS addresses and never follow redirects.
- Mojang publishes a SHA-1 hash for every game file, library, asset and Java
  runtime file. RustCraft checks each one after download, and again before
  each launch for files already on disk. A file that does not match is
  deleted and downloaded again. If the download still does not match, the
  launch stops.
- Downloads go to a `.part` file and are renamed only after the check
  passes, so a half-written file is never used.
- Mojang's version list, and Fabric's loader profiles, come without a hash.
  For these, RustCraft relies on HTTPS. It checks that a Fabric profile
  belongs to the requested game version before it saves it.

SHA-1 is not collision-resistant. RustCraft uses it because it is the only
hash Mojang publishes, and HTTPS protects the files in transit.

## Files from the internet and from you

RustCraft treats file names inside downloaded metadata and archives as
untrusted:

- Every path taken from metadata (libraries, assets, runtime files, version
  names) must be a plain relative path. Paths with `..`, absolute paths,
  drive letters and backslashes are refused.
- Native libraries are unpacked only from entries whose names stay inside
  the target folder. Other entries are skipped.
- Symbolic links in Java runtimes must point inside the runtime folder.
- World files (NBT) have limits on length and nesting depth, and a region
  chunk can be at most 16 MiB.
- Resource-pack files read from zips are limited to 8 MiB each, and the
  image decoder limits each image to 512 MiB of memory.

RustCraft never runs a JAR or any other downloaded file to inspect it. The
only downloaded programs it runs are Mojang's Java runtime and the game,
when you press Play.

## Your files

- RustCraft writes settings and account files atomically: a crash cannot
  leave a half-written file.
- If a settings file cannot be read, RustCraft renames it to
  `<name>.broken-<time>` and starts with defaults. It never deletes what you
  wrote.
- Before the RustCraft engine first changes a world's region file in a
  session, it copies the file to `rustcraft-backups/` in the world folder.
- Uninstalling RustCraft on Windows keeps your worlds, settings and
  downloads unless you agree to remove them. A silent uninstall always
  keeps them.

## Mods

RustCraft does not download, install or convert mods. It reads the
metadata of the jars you put into an instance's `mods` folder to check them
(see [MODS.md](MODS.md)): only named metadata files, with size limits,
straight from the archive. It never runs or unpacks them. The official game
loads the mods, and they run with the same rights as the game. RustCraft does not sandbox the Java game, just as other launchers
do not. Install mods only from sources you trust.

## Privacy

RustCraft has no telemetry, no analytics and no crash upload. It connects
only to:

- Microsoft, Xbox and Minecraft services, to sign in;
- Mojang's servers, for game files and Java runtimes;
- Fabric's metadata service (`meta.fabricmc.net`), only for Fabric instances;
- the server you choose to join, through the game.

Worlds, logs, crash reports, mods and screenshots stay on your computer.

## Privileges

RustCraft never asks for administrator rights. The Windows installer
installs for the current user only. The game runs as your user.

## Builds and dependencies

- `Cargo.lock` pins every dependency, and CI builds with `--locked`.
- CI runs `cargo audit` against the RustSec advisory database.
- Release builds are signed and notarized when signing credentials are
  configured for the repository. Builds without them are unsigned, so
  Windows SmartScreen and macOS Gatekeeper will warn before they run them.
