#!/bin/sh
# End-to-end check of the launcher/game lifecycle on a real window:
# the launcher starts a stand-in "game", closes its window once the game is
# running, waits for it to exit, then reopens and screenshots the result.
#
# Usage: scripts/lifecycle-smoke.sh <rustcraft binary> <output dir>
# Needs a display (on headless Linux: xvfb-run -a scripts/lifecycle-smoke.sh ...).
# Uses no network: every file it needs is created locally and verified by hash.
set -eu

BIN=$1
OUT=$2
mkdir -p "$OUT"
HOME_DIR=$(mktemp -d)
trap 'rm -rf "$HOME_DIR"' EXIT

sha1() { if command -v sha1sum >/dev/null; then sha1sum "$1" | cut -d' ' -f1; else shasum -a 1 "$1" | cut -d' ' -f1; fi; }
size() { wc -c < "$1" | tr -d ' '; }

# Stand-in for java: answers -version, then behaves like a game that starts
# and quits after two seconds.
FAKE_JAVA="$HOME_DIR/fake-java"
cat > "$FAKE_JAVA" <<'SH'
#!/bin/sh
if [ "$1" = "-version" ]; then
  echo 'openjdk version "21.0.1" 2023-10-17' >&2
  echo 'OpenJDK 64-Bit Server VM' >&2
  exit 0
fi
echo "Setting user: Tester"
sleep 2
exit 0
SH
chmod +x "$FAKE_JAVA"

V="$HOME_DIR/versions/rc-test"
mkdir -p "$V" "$HOME_DIR/assets/indexes" "$HOME_DIR/instances/Lifecycle"
printf 'stand-in client jar' > "$V/rc-test.jar"
printf '{"objects":{}}' > "$HOME_DIR/assets/indexes/rc-test.json"
cat > "$V/rc-test.json" <<JSON
{
  "id": "rc-test", "type": "release", "mainClass": "Fake",
  "minecraftArguments": "--username \${auth_player_name} --version \${version_name}",
  "javaVersion": { "component": "java-runtime-delta", "majorVersion": 21 },
  "assetIndex": { "id": "rc-test", "sha1": "$(sha1 "$HOME_DIR/assets/indexes/rc-test.json")",
    "size": $(size "$HOME_DIR/assets/indexes/rc-test.json"), "url": "https://example.invalid/index.json" },
  "downloads": { "client": { "sha1": "$(sha1 "$V/rc-test.jar")", "size": $(size "$V/rc-test.jar"),
    "url": "https://example.invalid/client.jar" } },
  "libraries": []
}
JSON
cat > "$HOME_DIR/instances/Lifecycle/instance.toml" <<TOML
game_version = "rc-test"
min_memory_mb = 256
max_memory_mb = 512
java_path = "$FAKE_JAVA"
created = 0
[loader]
kind = "vanilla"
TOML
cat > "$HOME_DIR/accounts.toml" <<TOML
active = "00000000000000000000000000000001"
[[accounts]]
id = "00000000000000000000000000000001"
name = "Tester"
verified_at = $(date +%s)
TOML
echo 'selected_instance = "Lifecycle"' > "$HOME_DIR/config.toml"

RUSTCRAFT_HOME="$HOME_DIR" "$BIN" --smoke-test "$OUT/lifecycle.png" --smoke-launch
LOG="$HOME_DIR/logs/launcher.log"
cp "$LOG" "$OUT/lifecycle-launcher.log"

fail() { echo "lifecycle smoke test FAILED: $1"; cat "$LOG"; exit 1; }
grep -q "closing launcher window" "$LOG" || fail "window did not close for gameplay"
grep -q "window closed; supervising Lifecycle" "$LOG" || fail "supervisor did not take over"
grep -q "Lifecycle closed normally" "$LOG" || fail "game exit not recorded"
[ -s "$OUT/lifecycle.png" ] || fail "window did not reopen and render"
grep -q "Setting user: Tester" "$HOME_DIR"/logs/Lifecycle-*.log || fail "game output not captured"
echo "lifecycle smoke test passed"
