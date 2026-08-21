<img width="300" height="100" alt="napstr-logo-small" src="https://github.com/user-attachments/assets/83c9ccea-3241-4fce-a7cf-16c83629484b" />

<img height="400" alt="image" src="https://github.com/user-attachments/assets/95b4d9f8-a844-49e1-ba9a-6dd003c8acd3" />

Napstr uses Nostr for discovery and Tor for private file sharing.

https://napstr.net

## Build from source

Install [Node.js](https://nodejs.org/), [Rust](https://rustup.rs/), and the
[Tauri prerequisites for your OS](https://v2.tauri.app/start/prerequisites/),
then:

```bash
git clone https://github.com/lnbits/napstr.git
cd napstr
npm ci
npm run desktop
```

Development builds use `NAPSTR_TOR_PATH` when set, then a bundled Tor binary, then `tor` on `PATH`.

On macOS, install Tor with Homebrew and start Napstr with:

```bash
brew install tor
NAPSTR_TOR_PATH="$(command -v tor)" npm run desktop
```

To create a package for your operating system with Tor included, run:

```bash
npm run bundle
```

Napstr automatically downloads and verifies the pinned official Tor Expert
Bundle for your platform before building.

## Build AppImage, DEB, and EXE installers

Release packages include a pinned official Tor Expert Bundle (version 15.0.20) whose SHA-256 is verified before packaging. The GitHub release workflow prepares the correct bundle and builds:

- Linux x86-64: `.AppImage` and `.deb`
- Windows x86-64: NSIS setup `.exe`

Trigger [the release workflow](.github/workflows/release.yml) manually, or push a version tag such as `v0.1.0`. Tagged outputs are attached to a draft GitHub release.
The workflow applies the tag version to the npm, Cargo, Tauri, installer, and in-app version metadata before building.

Tauri packages must be produced on their target operating system. For a local package, first run `scripts/prepare-tor.sh` with the platform's official archive URL and pinned hash from the release matrix, then run:

```bash
npm run bundle
```

Unsigned builds can trigger operating-system warnings. Public distribution requires a Windows code-signing certificate.

## Implemented architecture

- On first launch, Napstr creates a Nostr identity and securely stores its private key using your operating system's credential store.
- Nostr publishes the searchable catalogue and live seeders; NIP-17 handles private download negotiation.
- A bundled Tor process carries transfers without a direct-IP fallback.
- One recursively watched folder contains both downloads and shared audio.
- Files are audio-validated and identified by SHA-256. Downloads use a responsive seeder, verify the complete hash, and are available in the built-in player.

Profiles and catalogue metadata are public. Requests, transfer credentials, file contents, and peer IP addresses are not published. Tor use may still be visible to an ISP.
