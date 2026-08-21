#!/usr/bin/env bash
set -euo pipefail

input="${1:?usage: postprocess-appimage.sh INPUT.AppImage [OUTPUT.AppImage]}"
output="${2:-$input}"

if [[ ! -f "$input" ]]; then
  echo "AppImage not found: $input" >&2
  exit 1
fi

case "$output" in
  *.AppImage) ;;
  *)
    echo "Output must end in .AppImage: $output" >&2
    exit 1
    ;;
esac

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
input="$(realpath "$input")"
output_dir="$(cd "$(dirname "$output")" && pwd)"
output="$output_dir/$(basename "$output")"
work_dir="$(mktemp -d "${TMPDIR:-/tmp}/napstr-appimage.XXXXXXXX")"
trap 'rm -rf -- "$work_dir"' EXIT

(
  cd "$work_dir"
  "$input" --appimage-extract >/dev/null
)

app_dir="$work_dir/squashfs-root"
if [[ ! -x "$app_dir/usr/bin/napstr" ]]; then
  echo "The AppImage does not contain usr/bin/napstr" >&2
  exit 1
fi

# linuxdeploy intentionally excludes a number of libraries it expects every
# Linux distribution to provide. NixOS does not expose those libraries through
# the usual FHS paths. GStreamer's core needs zlib and Napstr's native player
# links the ALSA compatibility library even when PulseAudio is the selected
# output. Keep these stable dependencies inside the AppImage.
if [[ ! -e "$app_dir/usr/lib/libz.so.1" ]]; then
  zlib_path="$(ldconfig -p 2>/dev/null | awk '$1 == "libz.so.1" { print $NF; exit }')"
  if [[ -z "$zlib_path" || ! -e "$zlib_path" ]]; then
    echo "The build host does not provide libz.so.1 for the AppImage" >&2
    exit 1
  fi
  cp -L -- "$zlib_path" "$app_dir/usr/lib/libz.so.1"
  chmod 0644 "$app_dir/usr/lib/libz.so.1"
fi

if [[ ! -e "$app_dir/usr/lib/libasound.so.2" ]]; then
  alsa_path="$(ldconfig -p 2>/dev/null | awk '$1 == "libasound.so.2" { print $NF; exit }')"
  if [[ -z "$alsa_path" || ! -e "$alsa_path" ]]; then
    echo "The build host does not provide libasound.so.2 for the AppImage" >&2
    exit 1
  fi
  cp -L -- "$alsa_path" "$app_dir/usr/lib/libasound.so.2"
  chmod 0644 "$app_dir/usr/lib/libasound.so.2"
fi

gstreamer_plugins="$app_dir/usr/lib/gstreamer-1.0"
if [[ ! -f "$gstreamer_plugins/libgstautodetect.so" ]]; then
  echo "The AppImage is missing GStreamer's autoaudiosink plugin" >&2
  exit 1
fi
if [[ ! -f "$gstreamer_plugins/libgstpulseaudio.so" && ! -f "$gstreamer_plugins/libgstalsa.so" ]]; then
  echo "The AppImage is missing a GStreamer audio-output plugin" >&2
  exit 1
fi
if [[ ! -f "$app_dir/apprun-hooks/linuxdeploy-plugin-gstreamer.sh" ]]; then
  echo "The AppImage is missing its GStreamer runtime hook" >&2
  exit 1
fi

if command -v gst-inspect-1.0 >/dev/null 2>&1; then
  for element in \
    playbin filesrc typefind id3demux mpegaudioparse \
    audioconvert audioresample volume \
    autoaudiosink pulsesink alsasink \
    mpg123audiodec flacdec vorbisdec opusdec wavparse; do
    if ! env \
      APPDIR="$app_dir" \
      LD_LIBRARY_PATH="$app_dir/usr/lib:$app_dir/usr/lib/x86_64-linux-gnu" \
      GST_REGISTRY_1_0="$work_dir/gstreamer-registry.bin" \
      GST_REGISTRY_REUSE_PLUGIN_SCANNER=no \
      GST_PLUGIN_SYSTEM_PATH_1_0="$gstreamer_plugins" \
      GST_PLUGIN_PATH_1_0="$gstreamer_plugins" \
      GST_PLUGIN_SCANNER_1_0="$app_dir/usr/lib/gstreamer1.0/gstreamer-1.0/gst-plugin-scanner" \
      gst-inspect-1.0 "$element" >/dev/null; then
      echo "The AppImage cannot load the GStreamer element: $element" >&2
      exit 1
    fi
  done
fi

# linuxdeploy currently copies Ubuntu's Wayland client libraries into the
# AppImage. They conflict with the host Mesa stack and can leave WebKitGTK as a
# blank window (EGL_BAD_PARAMETER). Let the host supply this tightly coupled
# graphics family instead.
find "$app_dir/usr/lib" -maxdepth 1 \
  \( -type f -o -type l \) -name 'libwayland-*' -delete

install -m 0755 "$script_dir/appimage/AppRun" "$app_dir/AppRun"

repacked="$work_dir/$(basename "$output")"
architecture="${ARCH:-$(uname -m)}"
case "$architecture" in
  amd64) architecture=x86_64 ;;
  arm64) architecture=aarch64 ;;
esac

if [[ -n "${APPIMAGETOOL:-}" ]]; then
  ARCH="$architecture" "$APPIMAGETOOL" "$app_dir" "$repacked"
else
  plugin="${TAURI_APPIMAGE_PLUGIN:-${XDG_CACHE_HOME:-$HOME/.cache}/tauri/linuxdeploy-plugin-appimage.AppImage}"
  if [[ ! -x "$plugin" ]]; then
    echo "Tauri's AppImage plugin was not found at: $plugin" >&2
    exit 1
  fi

  (
    cd "$work_dir"
    ARCH="$architecture" \
      LDAI_OUTPUT="$repacked" \
      APPIMAGE_EXTRACT_AND_RUN=1 \
      "$plugin" --appdir="$app_dir"
  )
fi

if [[ ! -s "$repacked" ]]; then
  echo "AppImage repacking did not create: $repacked" >&2
  exit 1
fi

chmod 0755 "$repacked"
mv -f -- "$repacked" "$output"
echo "Repaired AppImage: $output"
