# Hearthsound MusicBrainz plugin

A [Hearthsound](https://github.com/hearthsound) metadata and artwork plugin
backed by the
[MusicBrainz web service](https://musicbrainz.org/doc/MusicBrainz_API) and the
[Cover Art Archive](https://coverartarchive.org). It compiles to a WebAssembly
component that implements the `metadata` and `artwork` provider interfaces, so
a Hearthsound server can resolve a recording hint (title, artists, album,
duration, ISRC) to scored MusicBrainz recordings, materialize a release, with
its track list, from MusicBrainz ids, read an artist's web links from its
MusicBrainz id or name, and name the renditions of an album's front cover.

The plugin needs no MusicBrainz account and no API key: every endpoint it calls
is part of the public web service, and the covers it names are part of the
Cover Art Archive. It never fetches image bytes itself, because the server
reads an image through the instance that named it.

## Requirements

- Rust 1.99 or newer, with the `wasm32-wasip2` target. The included
  `rust-toolchain.toml` pins both; `rustup toolchain install` fetches them.

## Build

```console
cargo build --release --locked --target wasm32-wasip2
```

The component lands at
`target/wasm32-wasip2/release/hearthsound_plugin_musicbrainz.wasm`. It exports
the `metadata` and `artwork` provider interfaces.

## Check and pack

Install the plugin CLI from the SDK repository:

```console
cargo install --locked --git https://github.com/hearthsound/plugin-sdk --tag v0.3.0 hearthsound-plugin-cli
```

Then validate the manifest and the component:

```console
hsplugin check
```

It prints `ok: app.hearthsound.musicbrainz 0.2.0 (provides metadata, artwork)`
on success.

Finally, build the archive you install on a server:

```console
hsplugin pack --out-dir dist
```

`dist/app.hearthsound.musicbrainz-0.2.0.hsplugin` contains the manifest and the
component.

## Tests

The query building, scoring, mapping and artwork lookup logic runs on the host
target, so its unit tests need no WebAssembly toolchain and no network:

```console
cargo test
```

## Configuration

| Instance setting | Default | Meaning |
|---|---|---|
| `baseUrl` | `https://musicbrainz.org/ws/2/` | Base URL of the MusicBrainz web service. Point it at a mirror to keep traffic off `musicbrainz.org`. |
| `artworkBaseUrl` | `https://coverartarchive.org/` | Base URL of the Cover Art Archive. Point it at a mirror to keep traffic off `coverartarchive.org`. |

A base URL that does not end in a slash is normalized to one, so the endpoint
paths the plugin appends always land on the service root. The manifest grants
the plugin the `musicbrainz.org` and Cover Art Archive hosts plus whichever
hosts the two settings name.

The instance requests a `host-http` rate limit of one request per second with a
burst of one, which is the average MusicBrainz publishes and covers the Cover
Art Archive requests too. The limit is enforced by the host, because exceeding
the average answers `503` to every caller of the IP, not only to the plugin
(https://musicbrainz.org/doc/MusicBrainz_API/Rate_Limiting).

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
the track's first ISRC. The album's images are empty, because the server asks
the artwork layer for a cover instead.

`get-artist-info` reads an artist's web pages. A request that carries an
`mb-artist` id is looked up directly through `GET /artist/{mbid}?inc=url-rels`;
a request that carries only a name goes through
`GET /artist?query=artist:"<name>"` and takes the first hit that spells the name
the same way, ignoring case, and that the index scored at least 90, so a
namesake, a tribute act or a loosely similar name is not mistaken for the
artist. The answer carries the `mb-artist` id and one link per mapped relation
(`official homepage` as `homepage`, plus `wikipedia`, `wikidata`, `last.fm`,
`discogs`, `allmusic` and `bandcamp`); every other relation type is dropped. The
biography is empty, because MusicBrainz publishes none. An artist that neither
an id nor a qualifying hit names is answered with no information rather than an
error.

`find-artwork` names the Cover Art Archive renditions of an album's front cover,
narrowest first: 250, 500 and 1200 pixels wide. The renditions follow what the
subject carries. An `mb-release` id has its cover at
`/release/<mbid>/front-<width>` and an `mb-release-group` id at
`/release-group/<mbid>/front-<width>`. A `upc` goes through
`GET /release?query=barcode:<upc>` and the release of the first result is used.
Anything else goes through
`GET /release-group?query=releasegroup:"<title>" AND artist:"<first artist>"`,
where the first hit the index scored at least 90 wins, so a similarly named
album is not mistaken for the one the subject names. No lookup that finds
nothing is an error: it is an empty list, and so is every artist subject,
because the Cover Art Archive holds no portraits. A lookup result is cached in
the instance's key-value store for 30 days, and a lookup that found nothing for
one day, so an album grid costs a request only the first time it is drawn.

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
| `src/lib.rs` | The plugin: configuration, lifecycle, and the metadata and artwork interfaces. |
| `src/api.rs` | The HTTP transport and MusicBrainz's error statuses. |
| `src/metadata.rs` | One function per metadata operation, over the API endpoints. |
| `src/artwork.rs` | The Cover Art Archive lookups and the cache of their results. |
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
