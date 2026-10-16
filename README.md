# Hearthsound MusicBrainz metadata plugin

A [Hearthsound](https://github.com/hearthsound) metadata plugin backed by the
[MusicBrainz web service](https://musicbrainz.org/doc/MusicBrainz_API). It
compiles to a WebAssembly component that implements the `metadata` provider
interface, so a Hearthsound server can resolve a recording hint (title,
artists, album, duration, ISRC) to scored MusicBrainz recordings and
materialize a release, with its track list, from MusicBrainz ids.

The plugin needs no MusicBrainz account and no API key: every endpoint it calls
is part of the public web service. It does not fetch artwork yet; the images of
a materialized release are empty until the artwork capability arrives.

## Requirements

- Rust 1.99 or newer, with the `wasm32-wasip2` target. The included
  `rust-toolchain.toml` pins both; `rustup toolchain install` fetches them.

## Build

```console
cargo build --release --locked --target wasm32-wasip2
```

The component lands at
`target/wasm32-wasip2/release/hearthsound_plugin_musicbrainz.wasm`.

## Check and pack

Install the plugin CLI from the SDK repository:

```console
cargo install --locked --git https://github.com/hearthsound/plugin-sdk --tag v0.2.0 hearthsound-plugin-cli
```

Then validate the manifest and the component:

```console
hsplugin check
```

It prints `ok: app.hearthsound.musicbrainz 0.1.0 (provides metadata)` on
success.

Finally, build the archive you install on a server:

```console
hsplugin pack --out-dir dist
```

`dist/app.hearthsound.musicbrainz-0.1.0.hsplugin` contains the manifest and the
component.

## Tests

The query building, scoring and mapping logic runs on the host target, so its
unit tests need no WebAssembly toolchain and no network:

```console
cargo test
```

## Configuration

| Instance setting | Default | Meaning |
|---|---|---|
| `baseUrl` | `https://musicbrainz.org/ws/2/` | Base URL of the MusicBrainz web service. Point it at a mirror to keep traffic off `musicbrainz.org`. |

A base URL that does not end in a slash is normalized to one, so the endpoint
paths the plugin appends always land on the web service root.

The instance requests a `host-http` rate limit of one request per second with a
burst of one, which is the average MusicBrainz publishes. The limit is enforced
by the host, because exceeding the average answers `503` to every caller of the
IP, not only to the plugin (https://musicbrainz.org/doc/MusicBrainz_API/Rate_Limiting).

## Behavior

`resolve-recording` resolves a recording hint to scored MusicBrainz recordings,
best score first:

- A hint that carries a usable ISRC (a bare 12-character code, once hyphens and
  whitespace are removed) is looked up through `GET /isrc/{isrc}`. The web
  service assigns an ISRC to one or a few recordings, so those hits start from a
  score of 1.0 and the recording id and the ISRC are reported in `ids`. The JSON
  serialization of an ISRC lookup carries no releases even when `inc=releases`
  asks for them, so a hit costs one more `GET /recording/{mbid}` to read its
  releases; when that request fails the match keeps its recording identity and
  an empty release list rather than failing the call.
- Any other hint, and an ISRC the web service does not know, goes through
  `GET /recording?query=...`: a Lucene query of `recording:"<title>"` plus the
  first artist, the album as `release:"<album>"`, and a `dur:[...]` window of
  plus or minus five seconds around the hint's duration. Every value is escaped,
  so a title full of Lucene operators cannot rewrite the query.

The score is `base * agreement`, where the base is 1.0 for an ISRC hit and
`score / 100` for a search hit, and the agreement is the weighted average of the
signals the hint carries: the title (weight 2, 1.0 for an equal title, 0.8 when
one contains the other, 0.3 otherwise) and the duration (weight 1, 1.0 within
2 s, 0.9 within 5 s, 0.5 within 15 s, 0 otherwise). A signal the hint does not
carry does not count, so an ISRC hit with a matching title scores exactly 1.0.
The server treats 0.9 or more as a confident first match.

`get-release` materializes an album from an `mb-release` id, or, when the caller
only has an `mb-release-group` id, from the earliest official release of that
group (an undated release sorts after every dated one). The album carries the
release id and title, its artists, release date, the primary type of its release
group, its barcode as the UPC, its genres (or the release group's when the
release has none), and every track of every medium with the medium position as
the disc number, the track position as the track number, the recording id and
the track's first ISRC. Images are empty: this plugin does not read artwork yet.

Errors follow the provider contract: `404` is a not-found, `429` and the `503`
MusicBrainz answers an exceeded request rate with are rate-limited (with the
`Retry-After` delay when the response carries one, and one second otherwise),
and every other failure is unavailable or internal.

## Notes

- The host adds `User-Agent: Hearthsound/<version> (+https://hearthsound.app)
  plugin/<id>/<version>` to every request the plugin does not label itself,
  which satisfies MusicBrainz's mandatory meaningful User-Agent rule.
- The service issues its own ids, so the plugin reports them as entity ids and
  lets the server namespace them per provider instance. The canonical
  identifiers MusicBrainz knows are reported in their own namespaces: `isrc`,
  `mb-recording`, `mb-release`, `mb-release-group`, `mb-artist` and `upc`.
- An ISRC is normalized before it is looked up (hyphens and whitespace removed,
  upper-cased), because MusicBrainz keys recordings by the bare 12-character
  code.
- The health probe reads one row of the genre table (`GET /genre/all?limit=1`),
  the smallest request the web service serves.
- A hint without a title is not searchable and resolves to no matches; that is
  an empty result, not an error.

## Layout

| Path | Contents |
|---|---|
| `Cargo.toml` | Crate metadata and the SDK dependency. |
| `src/lib.rs` | The plugin: configuration, lifecycle and the metadata interface. |
| `src/api.rs` | The HTTP transport and MusicBrainz's error statuses. |
| `src/metadata.rs` | One function per metadata operation, over the API endpoints. |
| `src/musicbrainz.rs` | Serde types for the web service responses. |
| `src/map.rs` | MusicBrainz records to provider records. |
| `src/query.rs` | Lucene query construction and escaping. |
| `src/score.rs` | Match scoring for recording candidates. |
| `src/url.rs` | Base URL validation and query encoding. |
| `src/fixtures/` | Captured web service responses the unit tests run on. |
| `manifest.json` | The plugin manifest the server reads. |

## License

Licensed under either of Apache License, Version 2.0 or MIT license at your
option.
