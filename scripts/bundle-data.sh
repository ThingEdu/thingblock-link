#!/usr/bin/env bash
# Build the self-contained arduino-cli data bundle in data/ (see the design doc).
#
# Installs the cores that ship pre-installed and refreshes the package indexes,
# using the shipped arduino-cli.yaml so the result lands in data/ rather than
# the user's ~/.arduino15. esp32 is deliberately NOT installed here — it
# downloads on demand via the board manager (the refreshed esp32 index is what
# makes that work offline-index-wise).
#
# Run once per target OS: the installed tools are native binaries.
#
# --with-esp32: also install esp32:esp32@3.3.11 (the core the ThingBot packs are tested against,
# ~1 GB), for an offline bundle: a classroom or competition venue with no Internet can then
# compile for ThingBot without the on-demand download.
set -euo pipefail

ESP32_CORE="esp32:esp32@3.3.11"
with_esp32=false
for arg in "$@"; do
    case "$arg" in
        --with-esp32) with_esp32=true ;;
        *) echo "unknown option: $arg" >&2; exit 1 ;;
    esac
done
cd "$(dirname "$0")/.."

case "$(uname -s)-$(uname -m)" in
    Darwin-*)      cli=arduino-cli-binaries/arduino-cli_mac_arm64/arduino-cli ;;
    Linux-aarch64) cli=arduino-cli-binaries/arduino-cli_linux_arm64/arduino-cli ;;
    Linux-*)       cli=arduino-cli-binaries/arduino-cli_linux_64bit/arduino-cli ;;
    MINGW*-* | MSYS*-* | CYGWIN*-*) cli=arduino-cli-binaries/arduino-cli_win_64bit/arduino-cli.exe ;;
    *) echo "unsupported host: $(uname -s)-$(uname -m)" >&2; exit 1 ;;
esac

"$cli" --config-file arduino-cli.yaml core update-index
"$cli" --config-file arduino-cli.yaml core install arduino:avr
if $with_esp32; then
    "$cli" --config-file arduino-cli.yaml core install "$ESP32_CORE"
fi

# Prune the download cache and temp files; the indexes and packages/ stay.
rm -rf data/staging data/tmp
# inventory.yaml carries a per-machine installation id/secret — never ship one.
rm -f data/inventory.yaml
