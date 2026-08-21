Release artifacts are no longer copied into the website. The download page discovers the
versioned installers attached to the latest published GitHub Release.

Create a `v*` tag to run `.github/workflows/release.yml`; the Tauri action builds and attaches
the Windows installer and Linux AppImage automatically.
