# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.5.0] - 2026-10-27

### Added

- The catalog layer: `search`, `get-track`, `get-album`, `get-artist`,
  `artist-albums`, `artist-top-tracks`, `lookup-isrc` and `lookup-album` over the
  MusicBrainz web service, with the artist's most played recordings read from
  ListenBrainz. A release search and a discography list one release per release
  group, represented by the group's earliest official release, and report the
  album type, the edition, the track count and the canonical ids of that
  release.
- Artist biographies: `get-artist-info` reads the artist's Wikidata item, the
  page its sitelinks name in the configured Wikipedia edition and that page's
  first paragraph, and reports the page as a link. The `wikipediaLanguage`
  instance setting picks the edition (default `en`).
- Artist portraits through the artwork interface: an artist subject is served
  from Wikimedia Commons at the file Wikidata names with property `P18`,
  1200 pixels wide.
- Per-host rate limits in the manifest: MusicBrainz and the configured base URL
  keep the published average of one request per second, and the other hosts
  share the instance-wide five per second.

### Changed

- Build against the `hearthsound:plugin@0.5.0` API (plugin-sdk v0.5.0) and
  export the `catalog_metadata_artwork_provider` world; `provides` is now
  catalog, metadata and artwork.
- The cache of resolved lookups and the cache of Wikidata and Wikipedia answers
  share one typed key-value store with the existing found and missing lifetimes.
- The endpoint paths, the release selection rules and the URL helpers moved
  into modules of their own, and the log macros are no-ops on the host target so
  the unit tests exercise the same code the component runs.
- A release listing entry and a release lookup both report the album reference
  of a record, and an album's `album_type` is the lowercase type the API
  documents.

## [0.4.0] - 2026-10-25

### Changed

- Build against the `hearthsound:plugin@0.4.0` API (plugin-sdk v0.4.0).

### Fixed

- The earliest official release of a release group is picked across the
  group's pages rather than from the first browse page: the browse reports its
  `release-count`, the scan pages through the group up to five pages of 100,
  and a group past that cap logs that the scan stopped there. A group whose
  earliest press is not on the first page no longer reports a later year.
- An ISRC lookup no longer carries an unbounded fan-out of release detail
  fetches: at most five hits are fetched, and the rest are read without their
  releases with a debug line naming them.
- A release detail fetch that fails because the web service is unavailable or
  rate limited now fails the lookup instead of being folded into a truncated
  match set; an error that concerns only that recording is still logged and
  skipped.

## [0.3.0] - 2026-10-23

### Changed

- The plugin builds against the 0.3.0 plugin API
  (`hearthsound:plugin@0.3.0`).

## [0.2.0] - 2026-10-16

### Added

- Artist information through the metadata interface: `get-artist-info` resolves
  an artist by an `mb-artist` id or by an exact-name search of the artist
  index, and reports the id with the artist's web links from the url relations
  MusicBrainz records.
- Cover art through the artwork interface: `find-artwork` names the Cover Art
  Archive renditions of an album's front cover, from a release or release group
  id the server holds or from a barcode or a title it looks up, and caches the
  lookup result in the instance's key-value store.

### Changed

- The plugin targets the 0.2.0 plugin API: the manifest declares
  `hearthsound:plugin@0.2.0` and `provides` metadata and artwork, the SDK
  dependency and the CI CLI install move to the SDK's `v0.2.0` tag, and the
  instance requests the Cover Art Archive hosts along with an `artworkBaseUrl`
  setting.

## [0.1.0] - 2026-10-13

### Added

- The repository scaffolding: the toolchain pin, the dual MIT/Apache-2.0 license
  files and this changelog.
- Serde types for the MusicBrainz web service responses, the Lucene query and
  match scoring of a recording hint, and the translation of recordings and
  their releases into metadata records, with the captured response fixtures
  its unit tests run on.
- The metadata interface over the MusicBrainz web service: recording resolution
  from an ISRC or a Lucene search, release materialization from a release or
  release group id, a health probe, and the manifest that declares them.
- A CI workflow that formats, lints, tests, builds, checks and packs the
  plugin, and uploads the archive.

[Unreleased]: https://github.com/hearthsound/plugin-musicbrainz/compare/v0.5.0...HEAD
[0.5.0]: https://github.com/hearthsound/plugin-musicbrainz/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/hearthsound/plugin-musicbrainz/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/hearthsound/plugin-musicbrainz/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/hearthsound/plugin-musicbrainz/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/hearthsound/plugin-musicbrainz/releases/tag/v0.1.0
