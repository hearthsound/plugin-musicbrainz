//! MusicBrainz metadata plugin: a Hearthsound metadata provider over the
//! MusicBrainz web service.
//!
//! The plugin needs no account and no key, because every endpoint it uses is
//! part of MusicBrainz's public web service. It resolves a recording hint to
//! scored MusicBrainz recordings and materializes a release from MusicBrainz
//! ids.
//!
//! The only instance setting is `baseUrl`, which defaults to the public web
//! service and can be pointed at a mirror.
//!
//! The resolution machinery sits in public modules ([`url`], [`query`],
//! [`score`], [`musicbrainz`], [`map`]); the binding-coupled glue that drives
//! it stays private.
//!
//! # Tests
//!
//! The pure logic is unit-tested on the host target:
//!
//! ```text
//! cargo test
//! ```
//!
//! Rustdoc examples in those modules are illustrative `text` blocks rather than
//! doctests: the crate is a `cdylib` for `wasm32-wasip2`, so there is no host
//! library target for a doctest to link against. Every example appears as an
//! assertion in the unit tests of the module that carries it.

#![forbid(unsafe_code)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod map;
pub mod musicbrainz;
pub mod query;
pub mod score;
pub mod url;
