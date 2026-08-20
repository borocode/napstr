<img width="300" height="100" alt="napstr-logo-small" src="https://github.com/user-attachments/assets/83c9ccea-3241-4fce-a7cf-16c83629484b" />

> Showcasing the power of Nostr as a discovery layer for apps.

Napstr is back!

There is no Napstr server, tracker, file store, TURN service, WebRTC transport, or direct-IP transfer fallback.

Napstr uses Nostr for discovery and Tor for private file sharing. Files are hashed chunked and pulled for different peers.

## Develop

Install the [Tauri prerequisites for your OS](https://v2.tauri.app/start/prerequisites/), then:

```bash
npm ci
npm run desktop
```

Development builds use `NAPSTR_TOR_PATH` when set, then a bundled Tor binary, then `tor` on `PATH`. The browser-only UI preview (`npm run dev`) uses sample data because browser pages cannot access the native database, keyring, Tor process, or local folders.

On NixOS, enter the included shell with `nix develop` first.

## Verify

```bash
npm run check
npm run build
cd src-tauri && cargo test
```

The Rust tests cover deterministic file/chunk hashing, verified reconstruction, protocol framing, NIP-17/NIP-44/NIP-59 gift-wrap confidentiality, and rejection of incorrect transfer capabilities. An ignored live test exercises Tor bootstrap, `ADD_ONION`, SOCKS transfer, and teardown against the real Tor network; the release transport was also validated with the pinned Linux expert bundle.

## Build AppImage, EXE, and DMG installers

Release packages include a pinned official Tor Expert Bundle (version 15.0.20) whose SHA-256 is verified before packaging. The GitHub release workflow prepares the correct bundle and builds:

- Linux x86-64: `.AppImage` and `.deb`
- Windows x86-64: NSIS setup `.exe`
- macOS Intel: `.dmg`
- macOS Apple Silicon: `.dmg`

Trigger [the release workflow](.github/workflows/release.yml) manually, or push a version tag such as `v0.1.0`. Tagged outputs are attached to a draft GitHub release.

Tauri packages must be produced on their target operating system. For a local package, first run `scripts/prepare-tor.sh` with the platform's official archive URL and pinned hash from the release matrix, then run:

```bash
npm run bundle
```

Unsigned builds can trigger operating-system warnings. Public distribution requires Apple Developer ID signing/notarization and a Windows code-signing certificate.

## Implemented architecture

- A dedicated Nostr identity is generated for Napstr and kept in the operating-system keyring. `NAPSTR_NSEC` can supply a dedicated identity in development.
- Kind `30421` parameterized-replaceable catalogue events publish signed file metadata, keyed by full-file SHA-256. Removed shares are withdrawn with replacement events.
- Kind `30422` expiring, parameterized-replaceable availability heartbeats identify profiles currently seeding each exact file ID. Search aggregates identical files by SHA-256 and displays live seeders and their Nostr profiles.
- Kind `10050` advertises the user's NIP-17 inbox relays. Download requests, refusals, temporary onion endpoints, and capabilities use NIP-17 gift wraps (NIP-44 encryption and NIP-59 wrapping) through `nostr-sdk`.
- A managed, bundled Tor process creates a fresh `ADD_ONION` service and random 256-bit capability for each accepted request. The service listens only on `127.0.0.1`; the control connection keeps it alive for at most 15 minutes. The downloader rejects every destination that is not `.onion`.
- The binary transfer protocol supports `HELLO`, `REQUEST_CHUNK`, `CHUNK_DATA`, `TRANSFER_COMPLETE`, and `CANCEL`. A peer can request only an indexed file ID—never a path.
- Files are streamed in 1 MiB chunks. Available exact-file seeders negotiate independently over NIP-17 and claim missing chunks from one shared scheduler, so one download can use several Tor peers concurrently. Failed claims return to the pool. Every chunk and the reconstructed file are SHA-256 verified; the SQLite ledger records which seeder supplied each chunk and `.napstr-parts` retains valid resume data until completion.
- The selected shared directory is watched recursively. Changes trigger re-indexing, catalogue replacement/withdrawal, and a fresh availability heartbeat while connected.
- The share boundary accepts only content-validated MP3, FLAC, PCM WAV, Ogg Vorbis, and Opus files. Embedded cover artwork is allowed, while extensions must match the codec and additional media streams, malformed containers, appended WAV payloads, and unsupported formats are rejected. Validation runs while indexing, publishing, serving, and after reconstruction.
- Local SHA-256 and Nostr-publisher blocks are enforced across discovery, requests, publication, and serving. Signed public NIP-56 reports identify the catalogue event, publisher, and exact-file hash.
- Play controls revalidate the exact SHA-256 audio file and open it in the operating system's default player, keeping untrusted codec decoding outside Napstr's key- and network-holding process.

Public information includes the Napstr profile, catalogue metadata, canonical file ID, and profiles advertising it. NIP-17 requests, onion endpoints, capabilities, contents, and peer IP addresses remain private. Tor use may still be visible to a user's ISP; Napstr does not claim perfect anonymity.

