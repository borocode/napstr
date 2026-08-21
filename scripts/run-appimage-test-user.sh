#!/usr/bin/env bash
set -euo pipefail

profile="${1:-}"
if [[ ! "$profile" =~ ^[A-Za-z0-9_-]{1,32}$ ]]; then
  echo "usage: npm run appimage:test-user -- PROFILE" >&2
  echo "PROFILE may contain 1-32 letters, numbers, hyphens, or underscores." >&2
  exit 1
fi

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
project_dir="$(cd "$script_dir/.." && pwd)"

appimage="${NAPSTR_APPIMAGE:-}"
if [[ -z "$appimage" ]]; then
  for bundle_dir in \
    "$project_dir/src-tauri/target/appimage-container/release/bundle/appimage" \
    "$project_dir/src-tauri/target/release/bundle/appimage"; do
    if [[ -d "$bundle_dir" ]]; then
      appimage="$(find "$bundle_dir" -maxdepth 1 -type f -name '*.AppImage' -print -quit)"
      [[ -n "$appimage" ]] && break
    fi
  done
fi

if [[ -z "$appimage" || ! -x "$appimage" ]]; then
  echo "No executable Napstr AppImage was found. Run npm run appimage:build first." >&2
  exit 1
fi

profile_root="$project_dir/.cache/test-users/$profile"
mkdir -p "$profile_root/data" "$profile_root/config"

launcher=("$appimage")
if command -v appimage-run >/dev/null 2>&1; then
  launcher=(appimage-run "$appimage")
elif [[ -r /etc/NIXOS ]]; then
  echo "appimage-run is required on NixOS; enter the project with nix develop." >&2
  exit 1
fi

echo "Starting isolated Napstr test user: $profile"
echo "Data: $profile_root/data/social.napstr.desktop"
echo "Identity: OS keyring account nostr-identity-$profile"

exec env \
  NAPSTR_PROFILE="$profile" \
  XDG_CONFIG_HOME="$profile_root/config" \
  XDG_DATA_HOME="$profile_root/data" \
  "${launcher[@]}"
