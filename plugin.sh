#!/bin/sh
here="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
sys="$(uname -s)"
case "$sys" in
MINGW* | MSYS* | CYGWIN*) rel="x86_64-pc-windows-msvc/gray-voicepro.exe" ;;
Darwin*)
    case "$(uname -m)" in
    arm64) rel="aarch64-apple-darwin/gray-voicepro" ;;
    *) rel="x86_64-apple-darwin/gray-voicepro" ;;
    esac ;;
*)
    case "$(uname -m)" in
    aarch64 | arm64) rel="aarch64-unknown-linux-musl/gray-voicepro" ;;
    *) rel="x86_64-unknown-linux-musl/gray-voicepro" ;;
    esac ;;
esac
if [ -x "$here/$rel" ]; then exec "$here/$rel" "$@"; fi
for cand in "$here/target/release/gray-voicepro.exe" "$here/target/release/gray-voicepro" "$here/target/debug/gray-voicepro.exe" "$here/target/debug/gray-voicepro"; do
    if [ -x "$cand" ]; then exec "$cand" "$@"; fi
done
echo "gray-voicepro binary not found in $here" >&2
exit 1
