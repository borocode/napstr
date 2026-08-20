#!/usr/bin/env bash
set -euo pipefail

platform="${1:?platform is required (linux, windows, or macos)}"
archive_url="${2:?official archive URL is required}"
expected_sha256="${3:?expected SHA-256 is required}"

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

if [[ "$platform" != "windows" ]]; then
  chmod +x "$destination/tor/tor"
fi
