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
cargo install --locked --git https://github.com/hearthsound/plugin-sdk --tag v0.1.1 hearthsound-plugin-cli
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
