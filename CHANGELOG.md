# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
