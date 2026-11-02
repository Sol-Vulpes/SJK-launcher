#!/bin/sh
# Builds both launchers in release mode and packs them into dist/: the SJK Launcher
# and the JoF Launcher (`--features jof`, built in target/jof so neither rebuilds
# the other).
#   sh build.sh          build and pack
#   sh build.sh --test   run the tests first (both looks)
set -eu
cd "$(dirname "$0")"

if [ "${1:-}" = "--test" ]; then
    cargo test --locked --workspace
    cargo test --locked -p sjk-launcher --features jof --target-dir target/jof
fi

case "$(uname -s)" in Darwin) os=macos ;; *) os=linux ;; esac
case "$(uname -m)" in arm64 | aarch64) arch=arm64 ;; *) arch=x64 ;; esac

# pack <program name> <target dir> <cargo flags...>
pack() {
    program="$1"
    target="$2"
    shift 2
    cargo build --locked --release -p sjk-launcher --target-dir "$target" "$@"
    name="$program-$os-$arch"
    stage="dist/$name"
    rm -rf "$stage" "dist/$name.tar.gz"
    mkdir -p "$stage"
    # The fonts are built into the binary; their licence asks to travel with them.
    cp "$target/release/sjk-launcher" "$stage/$program"
    cp assets/fonts/*-OFL.txt "$stage/"
    tar -czf "dist/$name.tar.gz" -C "$stage" "$program" Exo2-OFL.txt Rajdhani-OFL.txt
    echo "Built dist/$name.tar.gz"
}

pack sjk-launcher target
pack jof-launcher target/jof --features jof
