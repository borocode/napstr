#!/usr/bin/env bash
set -euo pipefail

platform="${1:?platform is required (linux, windows, or macos)}"
archive_url="${2:?official archive URL is required}"
expected_sha256="${3:?expected SHA-256 is required}"

case "$platform" in
  linux | windows | macos) ;;
  *)
    echo "Unsupported Tor bundle platform: $platform" >&2
    exit 1
    ;;
esac

archive="${RUNNER_TEMP:-/tmp}/napstr-tor-expert.tar.gz"
destination="src-tauri/resources/tor/${platform}"

curl --fail --location --silent --show-error "$archive_url" --output "$archive"
if command -v sha256sum >/dev/null 2>&1; then
  # Reading from stdin avoids the leading `\` that GNU sha256sum emits when
  # escaping Windows paths containing backslashes.
  actual_sha256="$(sha256sum < "$archive" | cut -d ' ' -f 1)"
else
  actual_sha256="$(shasum -a 256 < "$archive" | cut -d ' ' -f 1)"
fi
if [[ "$actual_sha256" != "$expected_sha256" ]]; then
  echo "Tor Expert Bundle SHA-256 mismatch" >&2
  echo "expected: $expected_sha256" >&2
  echo "actual:   $actual_sha256" >&2
  exit 1
fi

mkdir -p "$destination"
tar -xzf "$archive" -C "$destination"

# Napstr launches Tor itself and does not use the Expert Bundle's debug files,
# documentation, or optional bridge transports. Leaving those executable ELF
# files in the resource tree makes linuxdeploy inspect and strip programs that
# are not part of Napstr's runtime, and substantially inflates every installer.
for unused_path in \
  "$destination/debug" \
  "$destination/docs" \
  "$destination/tor/pluggable_transports"
do
  if [[ -e "$unused_path" ]]; then
    rm -rf -- "$unused_path"
  fi
done

tor_executable="$destination/tor/tor"
if [[ "$platform" == "windows" ]]; then
  tor_executable="$destination/tor/tor.exe"
fi

if [[ ! -f "$tor_executable" ]]; then
  echo "Tor Expert Bundle did not contain $tor_executable" >&2
  exit 1
fi

if [[ "$platform" != "windows" ]]; then
  chmod +x "$tor_executable"
fi
