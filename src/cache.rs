//! Typed access to the instance's key-value cache.
//!
//! Every answer this plugin caches is either a resolved value or the record
//! that a lookup resolved nothing, because "MusicBrainz does not know this
//! release" is as worth remembering as the release itself: without that record,
//! every album grid would ask the same question again. [`Cached`] carries both
//! states and decides how long the answer stays readable, and the [`Store`]
//! trait keeps the unit tests off the host's key-value import.
//!
//! A cache is an optimization, so a store that refuses to read or write costs a
//! log line and another request rather than a failed call; only a lookup the
//! cache misses reaches the network.

use serde::Serialize;
use serde::de::DeserializeOwned;

/// How long a resolved answer stays readable in the cache, in milliseconds.
///
/// What MusicBrainz, Wikidata and Wikipedia answer about an entity does not
/// change from day to day, so an answer is kept for 30 days.
pub const FOUND_TTL_MS: u64 = 30 * 24 * 60 * 60 * 1_000;

/// How long the record that a lookup resolved nothing stays readable, in
/// milliseconds.
///
/// An entity the indexes do not know yet may be added within days, so a
/// negative answer is only kept for one.
pub const MISSING_TTL_MS: u64 = 24 * 60 * 60 * 1_000;

/// A cached answer, resolved or empty.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case", tag = "state", content = "value")]
pub enum Cached<T> {
    /// The lookup resolved a value.
    Found(T),
    /// The lookup resolved nothing.
    Missing,
}

impl<T> Cached<T> {
    /// Returns how long this answer stays readable in the cache.
    ///
    /// # Returns
    ///
    /// [`FOUND_TTL_MS`] for a resolved value and [`MISSING_TTL_MS`] for the
    /// record that nothing was resolved.
    pub fn ttl_ms(&self) -> u64 {
        match self {
            Cached::Found(_) => FOUND_TTL_MS,
            Cached::Missing => MISSING_TTL_MS,
        }
    }

    /// Returns the resolved value, or `None` when the lookup resolved nothing.
    ///
    /// # Returns
    ///
    /// The value of a [`Cached::Found`] answer.
    pub fn value(self) -> Option<T> {
        match self {
            Cached::Found(value) => Some(value),
            Cached::Missing => None,
        }
    }
}

/// A key-value store the plugin caches answers in.
///
/// The host's key-value import implements this through [`InstanceStore`]; the
/// unit tests implement it with an in-memory map.
pub trait Store {
    /// Reads the entry stored under `key`.
    ///
    /// # Parameters
    ///
    /// - `key`: the key to read.
    ///
    /// # Returns
    ///
    /// The stored text, or `None` when the key is absent, has expired, or
    /// cannot be read.
    fn read(&self, key: &str) -> Option<String>;

    /// Stores `value` under `key`.
    ///
    /// # Parameters
    ///
    /// - `key`: the key to write.
    /// - `value`: the text to store.
    /// - `ttl_ms`: how long the entry stays readable, in milliseconds.
    fn write(&self, key: &str, value: &str, ttl_ms: u64);
}

/// The instance's key-value store, as the host provides it.
pub struct InstanceStore;

impl Store for InstanceStore {
    fn read(&self, key: &str) -> Option<String> {
        match hearthsound_plugin::host::kv::get(key) {
            Ok(Some(bytes)) => match String::from_utf8(bytes) {
                Ok(text) => Some(text),
                Err(error) => {
                    warn!("cache entry {key} is not text: {error}");

                    None
                }
            },
            Ok(None) => None,
            Err(reason) => {
                warn!("cache read of {key} failed: {reason}");

                None
            }
        }
    }

    fn write(&self, key: &str, value: &str, ttl_ms: u64) {
        if let Err(reason) = hearthsound_plugin::host::kv::set(key, value.as_bytes(), Some(ttl_ms))
        {
            warn!("cache write of {key} failed: {reason}");
        }
    }
}

/// Reads a cached answer.
///
/// # Parameters
///
/// - `store`: the store to read.
/// - `key`: the key to read.
///
/// # Returns
///
/// The cached answer, or `None` when the key is absent, expired, or holds a
/// value written by another version of the plugin. An unreadable value is a
/// miss, so a shape change is repaired by the next lookup rather than reported
/// as an error.
pub fn get<T: DeserializeOwned>(store: &dyn Store, key: &str) -> Option<Cached<T>> {
    let text = store.read(key)?;

    match serde_json::from_str(&text) {
        Ok(answer) => {
            debug!("cache entry {key} was used");

            Some(answer)
        }
        Err(error) => {
            debug!("cache entry {key} is unusable: {error}");

            None
        }
    }
}

/// Stores a cached answer for as long as its state allows.
///
/// # Parameters
///
/// - `store`: the store to write.
/// - `key`: the key to write.
/// - `answer`: the answer to store.
///
/// A write that cannot be serialized or that the store rejects is logged and
/// otherwise ignored, because the caller has the answer either way.
pub fn put<T: Serialize>(store: &dyn Store, key: &str, answer: &Cached<T>) {
    match serde_json::to_string(answer) {
        Ok(text) => store.write(key, &text, answer.ttl_ms()),
        Err(error) => warn!("cache entry {key} is not serializable: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use super::*;

    /// A store that keeps its entries in memory.
    struct MemoryStore {
        /// The entries the store holds.
        entries: RefCell<HashMap<String, String>>,
        /// The writes the store saw, in order.
        writes: RefCell<Vec<(String, String, u64)>>,
    }

    impl MemoryStore {
        /// Creates an empty store.
        fn empty() -> MemoryStore {
            MemoryStore {
                entries: RefCell::new(HashMap::new()),
                writes: RefCell::new(Vec::new()),
            }
        }
    }

    impl Store for MemoryStore {
        fn read(&self, key: &str) -> Option<String> {
            self.entries.borrow().get(key).cloned()
        }

        fn write(&self, key: &str, value: &str, ttl_ms: u64) {
            self.entries
                .borrow_mut()
                .insert(key.to_string(), value.to_string());
            self.writes
                .borrow_mut()
                .push((key.to_string(), value.to_string(), ttl_ms));
        }
    }

    #[test]
    fn a_resolved_answer_round_trips_through_the_store() {
        let store = MemoryStore::empty();

        put(&store, "k", &Cached::Found(7_u32));

        assert_eq!(get::<u32>(&store, "k").map(Cached::value), Some(Some(7)));
        assert_eq!(store.writes.borrow()[0].2, FOUND_TTL_MS);
    }

    #[test]
    fn a_missing_answer_round_trips_with_the_short_ttl() {
        let store = MemoryStore::empty();

        put(&store, "k", &Cached::<u32>::Missing);

        assert_eq!(get::<u32>(&store, "k").map(Cached::value), Some(None));
        assert_eq!(store.writes.borrow()[0].2, MISSING_TTL_MS);
    }

    #[test]
    fn an_absent_key_is_a_miss() {
        let store = MemoryStore::empty();

        assert!(get::<u32>(&store, "k").is_none());
    }

    #[test]
    fn an_unreadable_entry_is_a_miss() {
        let store = MemoryStore::empty();
        store
            .entries
            .borrow_mut()
            .insert("k".to_string(), "<html>".to_string());

        assert!(get::<u32>(&store, "k").is_none());
    }

    #[test]
    fn the_state_decides_the_ttl() {
        assert_eq!(Cached::Found(1_u8).ttl_ms(), 2_592_000_000);
        assert_eq!(Cached::<u8>::Missing.ttl_ms(), 86_400_000);
    }
}
