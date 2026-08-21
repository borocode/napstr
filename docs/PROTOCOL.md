# Napstr protocol 1

`napstr/1` is the interoperability and security boundary used by this implementation.

## Public Nostr events

Catalogue records are parameterized-replaceable kind `30421` events. Required tags are `d=<full SHA-256>`, `t=napstr`, `x=<full SHA-256>`, `name`, `size`, `m`, and `alt`. JSON content carries `protocol`, `fileId`, `filename`, `title`, `artist`, `album`, `format`, `mime`, `size`, `license`, `description`, and `tags` for protocol compatibility. Napstr sets `title` to the indexed filename and leaves the optional descriptive fields empty; the app has no separate catalogue-metadata editor. Publishing the same author/kind/`d` replaces a record. A removed share replaces it with a deletion document that cannot be parsed as a catalogue record.

Availability uses parameterized-replaceable kind `30422`, tagged with `d=availability-NNNN`, `t=napstr-availability`, and `expiration`. Its JSON content is an array of no more than 400 exact file IDs. Napstr sends fresh batches every four minutes with a ten-minute expiry and accepts only recent heartbeats. Replaceable events are used so relays retain the current heartbeat; ephemeral event kinds would normally not be stored and could not support on-demand search.

NIP-17 inbox relay lists use kind `10050` with `relay` tags. Profile metadata uses standard kind `0`.

Catalogue events never contain an onion, capability, local path, or transfer request.

## Private signalling

Every signal is a kind `14` private message delivered as a NIP-59 gift wrap by NIP-17. `nostr-sdk` applies NIP-44 encryption. Rumors carry NIP-40 `expiration` (20 minutes); Napstr rejects signals without a future expiry.

Messages are JSON with one of these types:

```text
DOWNLOAD_REQUEST { protocol, request_id, file_id }
DOWNLOAD_OFFER   { protocol, offer: { requestId, fileId, onion, port, capability, expiresAt } }
DOWNLOAD_REFUSED { protocol, request_id, file_id, reason }
```

The receiver accepts an offer only when the authenticated gift-wrap sender is one of the exact sources it requested. Capabilities are random 256-bit values, are stored only as SHA-256 lookups by the serving peer, and expire after 15 minutes.

## Onion transfer framing

Tor exposes a randomly generated v3 onion at virtual port 80 and forwards it only to a random `127.0.0.1` listener. The onion key is discarded and its authenticated control connection is retained only for the offer lifetime. Downloaders reject non-`.onion` hosts; there is no other connector.

Control messages are a big-endian 32-bit JSON length followed by UTF-8 JSON, capped at 64 KiB. The binary transfer protocol version is `2`.

```text
client HELLO             { version, capability, file_id }
server WELCOME           { version, file_id, filename, size }
client REQUEST_FILE
server FILE_DATA         { size, sha256 }
server raw bytes         exactly `size` bytes
client TRANSFER_COMPLETE
server TRANSFER_COMPLETE
client CANCEL
server ERROR             { code, message }
```

The server resolves only `file_id` in the indexed `files` table and streams no more than the indexed byte length. The downloader races up to three valid onion offers, selects the first responsive source, and writes one continuous temporary file. If that source fails, a standby restarts the stream. The file is accepted only when its final SHA-256 equals `file_id`.
