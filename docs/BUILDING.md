# Building RustCraft

## Requirements

- **Rust 1.97.0.** `rust-toolchain.toml` pins it, so `rustup` installs it
  automatically on the first build.
- **A C compiler** (for the `ring` crypto library): Visual Studio Build
  Tools on Windows, Xcode command-line tools on macOS, `gcc` or `clang` on
  Linux.

Building needs no other system libraries. At run time, Linux needs:

| Package (Debian/Ubuntu name) | Why |
|---|---|
| `libxkbcommon-x11-0` | Keyboard input on X11 |
| `libgl1-mesa-dri`, `mesa-vulkan-drivers` (or your GPU vendor's driver) | Graphics |
| A Secret Service, such as `gnome-keyring` | Saving the sign-in (optional) |

## Build

```sh
cargo build --release -p rustcraft-launcher -p rustcraft-client
```

This builds two programs in `target/release/`:

- `rustcraft` (`rustcraft.exe`): the launcher.
- `rustcraft-client`: the RustCraft engine. The launcher looks for it in
  its own folder.

To build in Microsoft sign-in, set the Azure client ID at build time (see
[AUTHENTICATION.md](AUTHENTICATION.md)):

```sh
RUSTCRAFT_MSA_CLIENT_ID=<application id> cargo build --release -p rustcraft-launcher
```

Development builds (`cargo build`) are optimised enough to play: the
workspace builds its own code at `opt-level = 1` and dependencies at
`opt-level = 2`, without debug information for dependencies.

## Run

```sh
target/release/rustcraft                     # the launcher
target/release/rustcraft-client --help       # the engine on its own
```

Useful settings:

| Variable or flag | Effect |
|---|---|
| `RUSTCRAFT_HOME=<dir>` | Use `<dir>` as the data folder, for example to keep tests apart from your real data |
| `RUSTCRAFT_MSA_CLIENT_ID=<id>` | The Azure client ID, at run time |
| `RUST_LOG=debug` | More detail in the logs |
| `rustcraft --smoke-test <png> [--smoke-tab <name>] [--smoke-launch]` | Open the launcher, save a screenshot and exit; with `--smoke-launch`, first play the selected instance |
| `rustcraft-client --screenshot <png>` | Load a world, save a screenshot and exit |
| `rustcraft-client --benchmark <seconds>` | See [BENCHMARKS.md](BENCHMARKS.md) |

## Test

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

The tests use no network: services are replaced by local mock servers.
Some tests use the real system when you ask for it:

| Command | What it checks |
|---|---|
| `RUSTCRAFT_TEST_KEYRING=1 cargo test -p rustcraft-auth os_keyring` | Saving and deleting a secret in the real credential store. Linux needs an unlocked Secret Service |
| `RUSTCRAFT_REQUIRE_GPU=1 cargo test -p rustcraft-renderer` | Rendering a world off screen. Without the variable, the test passes quietly when there is no GPU. `RUSTCRAFT_SHOT=<png>` saves the image |
| `xvfb-run -a scripts/lifecycle-smoke.sh target/debug/rustcraft <out dir>` | The whole launcher lifecycle in a real window: a stand-in Java game, then the RustCraft engine. Build both programs first |

CI (`.github/workflows/ci.yml`) runs formatting, Clippy, the tests on
Linux, Windows and macOS, the lifecycle test, the keyring test and
`cargo audit` on every push.

## Package

`.github/workflows/release.yml` builds, packages and smoke-tests all three
platforms. It runs on version tags (`v*`) and on manual dispatch, and it
publishes a draft GitHub release with `SHA256SUMS.txt`. The steps can also
run locally:

| Platform | Command | Output |
|---|---|---|
| Linux | `scripts/package-linux.sh [path/to/appimagetool]` | `dist/RustCraft-<version>-x86_64.AppImage` |
| macOS | `scripts/package-macos.sh`, after release builds for `aarch64-apple-darwin` and `x86_64-apple-darwin` | Universal `dist/RustCraft.app` and `dist/RustCraft-<version>-macos.dmg` |
| Windows | `makensis /DVERSION=<version> /DSRC=<dir with the .exe files> /DOUT=<setup.exe> packaging\windows\installer.nsi` | A per-user installer |

Signing:

- **macOS.** `package-macos.sh` signs with `$MACOS_SIGNING_IDENTITY` (a
  Developer ID with the hardened runtime). Without it, it signs ad hoc. The
  release workflow notarizes the dmg when the Apple ID secrets are set.
- **Windows.** The release workflow signs with `signtool` when
  `WINDOWS_CERT_PFX` and `WINDOWS_CERT_PASSWORD` are set.

Unsigned builds work, but Windows SmartScreen and macOS Gatekeeper warn
before they open them.

`scripts/make-icons.py` (needs Pillow) draws the app icons in
`packaging/icons` again.

## Repository layout

See [ARCHITECTURE.md](ARCHITECTURE.md).
