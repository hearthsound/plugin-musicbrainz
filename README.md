# Hearthsound MusicBrainz plugin

A [Hearthsound](https://github.com/hearthsound) catalog, metadata and artwork
plugin backed by the
[MusicBrainz web service](https://musicbrainz.org/doc/MusicBrainz_API), the
[Cover Art Archive](https://coverartarchive.org), Wikidata, Wikipedia and
[ListenBrainz](https://listenbrainz.org). It compiles to a WebAssembly component
that implements the `catalog`, `metadata` and `artwork` provider interfaces, so
a Hearthsound server can search MusicBrainz, resolve a recording hint (title,
artists, album, duration, ISRC) to scored MusicBrainz recordings, materialize a
release with its track list, read an artist's discography, most played tracks,
web links and biography, and name the renditions of an album's front cover and
of an artist's portrait.

The plugin needs no MusicBrainz account and no API key: every endpoint it calls
is part of a public service, and the images it names are served by the Cover Art
Archive and Wikimedia Commons. It never fetches image bytes itself, because the
server reads an image through the instance that named it.

## Requirements

- Rust 1.99 or newer, with the `wasm32-wasip2` target. The included
  `rust-toolchain.toml` pins both; `rustup toolchain install` fetches them.

## Build

```console
cargo build --release --locked --target wasm32-wasip2
```

The component lands at
`target/wasm32-wasip2/release/hearthsound_plugin_musicbrainz.wasm`. It exports
the `catalog`, `metadata` and `artwork` provider interfaces.

## Check and pack

Install the plugin CLI from the SDK repository:

```console
cargo install --locked --git https://github.com/hearthsound/plugin-sdk --tag v0.5.0 hearthsound-plugin-cli
```

Then validate the manifest and the component:

```console
hsplugin check
```

It prints `ok: app.hearthsound.musicbrainz 0.5.1 (provides catalog, metadata,
artwork)` on success.

Finally, build the archive you install on a server:

```console
hsplugin pack --out-dir dist
```

`dist/app.hearthsound.musicbrainz-0.5.1.hsplugin` contains the manifest and the
component.

## Tests

The query building, scoring, listing selection, mapping and cache logic runs on
the host target, so its unit tests need no WebAssembly toolchain and no network:

```console
cargo test
```

## Configuration

| Instance setting | Default | Meaning |
|---|---|---|
| `baseUrl` | `https://musicbrainz.org/ws/2/` | Base URL of the MusicBrainz web service. Point it at a mirror to keep traffic off `musicbrainz.org`. |
| `artworkBaseUrl` | `https://coverartarchive.org/` | Base URL of the Cover Art Archive. Point it at a mirror to keep traffic off `coverartarchive.org`. |
| `wikipediaLanguage` | `en` | Language code of the Wikipedia edition artist biographies are read from, for example `de` or `pt`. |
| `listenbrainzToken` | empty | Token of a ListenBrainz account, sent as `Authorization: Token <token>` when the artist's top tracks are read. Without one the plugin omits top tracks. |

A base URL that does not end in a slash is normalized to one, so the endpoint
paths the plugin appends always land on the service root. A language code must
be two or three lowercase letters. The ListenBrainz token is stored with the
instance's secrets, is never read back by the server, and may not contain a
control character, which no header value may.

The manifest grants the plugin `musicbrainz.org`, the Cover Art Archive,
`archive.org`, Wikidata, any Wikipedia edition, Wikimedia Commons,
`upload.wikimedia.org` and `api.listenbrainz.org`, plus whichever hosts the two
URL settings name.

MusicBrainz publishes an average of one request per second, so the manifest
gives `musicbrainz.org` and the configured `baseUrl` a limit of one request per
second with a burst of one, and every other host the instance-wide limit of five
per second with a burst of five. The limits are enforced by the host, because
exceeding MusicBrainz's average answers `503` to every caller of the IP, not
only to the plugin (https://musicbrainz.org/doc/MusicBrainz_API/Rate_Limiting).

## Behavior

### Catalog

`search` asks the indexes of the kinds the caller named and answers the page the
caller asked for: `GET /recording?query=...`, `GET /release?query=...
&inc=release-groups` and `GET /artist?query=...`. A track points at the first
official release the hit listed, an artist carries no images (a portrait per hit
would cost a Wikidata request), and the release hits are reduced to one release
per release group, keeping the group's earliest dated official release, because
the index answers with every pressing of an album. An empty text or a zero limit
sends no request.

`get-track` reads `GET /recording/{mbid}?inc=artist-credits+releases+isrcs` and
points the track at the earliest official release of its release list.
`get-album` materializes a release with its track list, exactly as
`get-release` does below. `get-artist` reads
`GET /artist/{mbid}?inc=url-rels`, adds the portrait Wikidata names for the
artist, and reports the album count an earlier `artist-albums` call counted; it
never counts the discography itself, because the caller asks for that
separately.

`artist-albums` browses `GET /release?artist={id}&status=official&
type=album|ep|single&inc=release-groups+artist-credits+media` page by page,
because the web service can only page releases and not release groups. Each
release group is represented by its earliest official release, and the answer is
sliced to the page the caller asked for. One page holds 100 releases and the
scan reads at most ten pages, so a request for a huge discography stays bounded.
When the listing was read to its end the number of release groups is cached, so
the next `get-artist` reports it.

`artist-top-tracks` reads the artist's most played recordings from
`GET https://api.listenbrainz.org/1/popularity/top-recordings-for-artist/{mbid}`
and slices the list to the caller's limit. A recording ListenBrainz does not tie
to a release is reported without an album. That endpoint answers only for a
ListenBrainz account, so the request carries `Authorization: Token <token>` from
the instance's `listenbrainzToken`; an instance without a token reports no top
tracks rather than an error, and a refused token is reported as `Unauthorized`.

`lookup-isrc` resolves an ISRC to the recordings assigned to it, and
`lookup-album` resolves canonical ids to the albums the web service holds: an
`mb-release` id names a release, an `mb-release-group` id resolves to the
group's earliest official release, and a `upc` searches
`GET /release?query=barcode:<upc>` and takes the first hit. An id in another
namespace is skipped, an album the service does not know is skipped rather than
failing the call, and the answer holds one reference per album in the order of
the ids.

### Album type and edition

An album's type is lowercase and follows its release group: `album`, `ep` or
`single` from the group's primary type, unless the group carries a secondary type
(`compilation`, `live`, `remix`, `soundtrack`, `dj-mix`, `demo`, `broadcast` or
`other`), which wins, because a compilation or a live album is what the listener
needs to see. A primary type outside those three names is reported as `other`.
An album's edition is the web service's own disambiguation of the pressing, when
it has one, which is the only way MusicBrainz names an edition.

### Metadata

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
release id and title, its artists, release date, its album type, its edition,
its barcode as the UPC, its genres (or the release group's when the release has
none), and every track of every medium with the medium position as the disc
number, the track position as the track number, the recording id and the track's
first ISRC. The album's images are empty, because the server asks the artwork
layer for a cover instead.

`get-artist-info` reads an artist's web pages and its biography. A request that
carries an `mb-artist` id is looked up directly through
`GET /artist/{mbid}?inc=url-rels`; a request that carries only a name goes
through `GET /artist?query=artist:"<name>"` and takes the first hit that spells
the name the same way, ignoring case, and that the index scored at least 90, so
a namesake, a tribute act or a loosely similar name is not mistaken for the
artist. The answer carries the `mb-artist` id and one link per mapped relation
(`official homepage` as `homepage`, plus `wikipedia`, `wikidata`, `last.fm`,
`discogs`, `allmusic` and `bandcamp`); every other relation type is dropped. The
biography is the first paragraph of the artist's Wikipedia page: the `wikidata`
url relation names the artist's Wikidata item, the item's sitelink names the page
in `wikipediaLanguage` (falling back to the English sitelink and then to the
`wikipedia` url relation), and `GET /api/rest_v1/page/summary/<title>` on that
edition carries the text. The page is added as a `wikipedia` link when the url
relations did not already name it. An artist that neither an id nor a qualifying
hit names is answered with no information rather than an error.

### Artwork

`find-artwork` names the Cover Art Archive renditions of an album's front cover,
narrowest first: 250, 500 and 1200 pixels wide. The renditions follow what the
album subject carries. An `mb-release` id has its cover at
`/release/<mbid>/front-<width>` and an `mb-release-group` id at
`/release-group/<mbid>/front-<width>`. A `upc` goes through
`GET /release?query=barcode:<upc>` and the release of the first result is used.
Anything else goes through
`GET /release-group?query=releasegroup:"<title>" AND artist:"<first artist>"`,
where the first hit the index scored at least 90 wins, so a similarly named
album is not mistaken for the one the subject names.

An artist subject is served from Wikimedia Commons: the `mb-artist` id (or the
instance's own id for the artist) reads the artist's url relations, their
`wikidata` relation names the item, the item's `P18` claim names the file, and
Commons serves it at
`https://commons.wikimedia.org/wiki/Special:FilePath/<file>?width=1200`. A track
or playlist subject is answered with an empty list.

No lookup that finds nothing is an error: it is an empty list. A lookup result
is cached in the instance's key-value store for 30 days, a lookup that found
nothing for one day, and so are the Wikidata entity and Wikipedia summary of an
artist, so an album grid or an artist page costs a request only the first time
it is read.

### Errors

Errors follow the provider contract: `404` is a not-found, `429` and the `503`
MusicBrainz answers an exceeded request rate with are rate-limited (with the
`Retry-After` delay when the response carries one, and one second otherwise),
and every other failure is unavailable or internal. A biography or portrait
that fails for a reason other than the service being unavailable costs that
part of the answer and is logged; the identity and the links are still
reported.

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
- An instance without a `listenbrainzToken` reports no top tracks instead of a
  failure, because ListenBrainz's popularity API answers only for an account.
- The module-level log macros are no-ops on the host target, which has no
  `host-log` import, so the unit tests exercise the same code the component runs.

## Layout

| Path | Contents |
|---|---|
| `Cargo.toml` | Crate metadata and the SDK dependency. |
| `src/lib.rs` | The plugin: configuration, lifecycle, and the catalog, metadata and artwork interfaces. |
| `src/api.rs` | The HTTP transport and MusicBrainz's error statuses. |
| `src/catalog.rs` | One function per catalog operation. |
| `src/metadata.rs` | One function per metadata operation. |
| `src/artwork.rs` | The album cover and artist portrait lookups. |
| `src/cache.rs` | The typed key-value cache and its time to live. |
| `src/wikimedia.rs` | Wikidata entities and Wikipedia page summaries. |
| `src/paths.rs` | The path of every web service endpoint. |
| `src/select.rs` | Which release of a listing the plugin reports. |
| `src/musicbrainz.rs` | Serde types for the web service responses. |
| `src/listenbrainz.rs` | Serde types for the popularity endpoint. |
| `src/map.rs` | MusicBrainz records to provider records. |
| `src/query.rs` | Lucene query construction and escaping. |
| `src/score.rs` | Match scoring for recording candidates. |
| `src/url.rs` | Base URL validation, URL helpers and encoding. |
| `src/fixtures/` | Captured web service responses the unit tests run on. |
| `manifest.json` | The plugin manifest the server reads. |

## License

Licensed under either of Apache License, Version 2.0 or MIT license at your
option.
