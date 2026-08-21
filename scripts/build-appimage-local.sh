#!/usr/bin/env bash
set -euo pipefail

mode="${1:-build}"
case "$mode" in
  build | run | launch) ;;
  *)
    echo "usage: build-appimage-local.sh [build|run|launch]" >&2
    exit 1
    ;;
esac

if [[ "$(uname -s)" != "Linux" ]]; then
  echo "AppImages must be built on Linux." >&2
  exit 1
fi

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
project_dir="$(cd "$script_dir/.." && pwd)"
cd "$project_dir"

launch_appimage() {
  local appimage="$1"
  if command -v appimage-run >/dev/null 2>&1; then
    appimage-run "$appimage"
  else
    "$appimage"
  fi
}

if [[ "$mode" == "launch" ]]; then
  if [[ -r /etc/NIXOS ]]; then
    target_dir="$project_dir/src-tauri/target/appimage-container"
  else
    target_dir="${CARGO_TARGET_DIR:-$project_dir/src-tauri/target}"
  fi
  appimage="$(find "$target_dir/release/bundle/appimage" \
    -maxdepth 1 -type f -name '*.AppImage' -print -quit 2>/dev/null || true)"
  if [[ -z "$appimage" ]]; then
    echo "No locally built AppImage was found. Run npm run appimage:build first." >&2
    exit 1
  fi
  launch_appimage "$appimage"
  exit 0
fi

if [[ -r /etc/NIXOS && -z "${NAPSTR_APPIMAGE_CONTAINER:-}" ]]; then
  if ! command -v docker >/dev/null 2>&1; then
    echo "Docker is required to build a portable AppImage from NixOS." >&2
    exit 1
  fi

  image="napstr-appimage-builder:ubuntu-22.04"
  echo "Building the Ubuntu 22.04 AppImage toolchain image..."
  docker build \
    --file "$project_dir/packaging/appimage.Dockerfile" \
    --tag "$image" \
    "$project_dir/packaging"

  mkdir -p \
    "$project_dir/.cache/appimage/cargo" \
    "$project_dir/.cache/appimage/npm" \
    "$project_dir/.cache/appimage/tauri" \
    "$project_dir/src-tauri/target/appimage-container"

  echo "Building Napstr in the Ubuntu 22.04 container..."
  docker run --rm \
    --user "$(id -u):$(id -g)" \
    --env HOME=/tmp/napstr-builder \
    --env CARGO_HOME=/workspace/.cache/appimage/cargo \
    --env CARGO_TARGET_DIR=/workspace/src-tauri/target/appimage-container \
    --env NAPSTR_APPIMAGE_CONTAINER=1 \
    --env npm_config_cache=/workspace/.cache/appimage/npm \
    --env XDG_CACHE_HOME=/workspace/.cache/appimage \
    --volume "$project_dir:/workspace" \
    --workdir /workspace \
    "$image" \
    bash -lc 'mkdir -p "$HOME" && npm ci && bash scripts/build-appimage-local.sh build'

  appimage="$(find "$project_dir/src-tauri/target/appimage-container/release/bundle/appimage" \
    -maxdepth 1 -type f -name '*.AppImage' -print -quit)"
  if [[ -z "$appimage" ]]; then
    echo "The container completed without producing an AppImage." >&2
    exit 1
  fi

  echo
  echo "AppImage ready: $appimage"
  if [[ "$mode" == "run" ]]; then
    echo "Launching the packaged application through NixOS appimage-run..."
    launch_appimage "$appimage"
  fi
  exit 0
fi

tor_url="https://archive.torproject.org/tor-package-archive/torbrowser/15.0.20/tor-expert-bundle-linux-x86_64-15.0.20.tar.gz"
tor_sha256="3b39a2a7fbf43ef28b9ae0a6afca02a12935232f81769e4fef7472d6b5676eaf"

echo "Preparing the pinned Tor Expert Bundle..."
bash "$script_dir/prepare-tor.sh" linux "$tor_url" "$tor_sha256"

echo "Building the Linux AppImage..."
tor_library_dir="$project_dir/src-tauri/resources/tor/linux/tor"
APPIMAGE_EXTRACT_AND_RUN=1 \
  NO_STRIP=1 \
  LD_LIBRARY_PATH="$tor_library_dir${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" \
  npm run tauri -- build --verbose --bundles appimage

target_dir="${CARGO_TARGET_DIR:-$project_dir/src-tauri/target}"
appimage="$(find "$target_dir/release/bundle/appimage" \
  -maxdepth 1 -type f -name '*.AppImage' -print -quit)"
if [[ -z "$appimage" ]]; then
  echo "Tauri completed without producing an AppImage." >&2
  exit 1
fi

if [[ -r /etc/NIXOS ]]; then
  # Tauri's cached plugin contains a native appimagetool, but its convenience
  # wrapper assumes /bin/bash. Extract and call the native tool directly so the
  # exact same post-processing also works on NixOS.
  plugin="${TAURI_APPIMAGE_PLUGIN:-${XDG_CACHE_HOME:-$HOME/.cache}/tauri/linuxdeploy-plugin-appimage.AppImage}"
  if [[ ! -x "$plugin" ]]; then
    echo "Tauri's AppImage plugin was not found at: $plugin" >&2
    exit 1
  fi
  tool_dir="$(mktemp -d "${TMPDIR:-/tmp}/napstr-appimagetool.XXXXXXXX")"
  trap 'rm -rf -- "$tool_dir"' EXIT
  (
    cd "$tool_dir"
    "$plugin" --appimage-extract >/dev/null
  )
  APPIMAGETOOL="$tool_dir/squashfs-root/appimagetool-prefix/AppRun" \
    bash "$script_dir/postprocess-appimage.sh" "$appimage"
else
  bash "$script_dir/postprocess-appimage.sh" "$appimage"
fi

echo
echo "AppImage ready: $appimage"

if [[ "$mode" == "run" ]]; then
  echo "Launching the packaged application..."
  launch_appimage "$appimage"
fi
