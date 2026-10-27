//! Selection rules over MusicBrainz listings.
//!
//! A release group lists its releases in the order the web service chose, which
//! is neither chronological nor by relevance, and a release search answers with
//! every pressing of an album rather than with the album. The rules here pick
//! the release the plugin reports: the earliest official one for a group, the
//! first official one a recording listing returned, and one representative per
//! release group for a listing. They are pure and unit-tested on the host
//! target.

use hearthsound_plugin::types::ProviderError;

use crate::musicbrainz;

/// Number of browse pages the earliest-release scan reads at most.
///
/// A release group rarely holds more than one page of official releases, and
/// every further page is another request at the web service's 1 request/s
/// average. A group with more releases than this many pages hold is scanned on
/// its first pages only, and the scan reports that the cap stopped it.
pub const EARLIEST_RELEASE_PAGES: u32 = 5;

/// Returns whether the web service reports a release as official.
///
/// # Parameters
///
/// - `release`: the release listing entry to read.
///
/// # Returns
///
/// `true` when the status is exactly `Official`, which excludes promotions,
/// bootlegs and pseudo-releases.
pub fn is_official(release: &musicbrainz::ReleaseStub) -> bool {
    release.status.as_deref() == Some("Official")
}

/// Picks the earliest release of a listing.
///
/// The release of a group that came first is the one the web service lists
/// first, so a date only becomes the deciding factor when there is one. A
/// release the web service left undated sorts after every dated one, because an
/// unknown date cannot be shown to be early.
///
/// # Parameters
///
/// - `releases`: the release listing entries to choose from.
///
/// # Returns
///
/// The earliest release, or `None` when the listing is empty.
///
/// # Example
///
/// ```text
/// earliest(&[dated("2002"), dated("1998-04-20"), dated("1998-09-22")])
///   -> the release dated "1998-04-20"
/// ```
pub fn earliest(releases: &[musicbrainz::ReleaseStub]) -> Option<&musicbrainz::ReleaseStub> {
    earliest_of(releases.iter())
}

/// Picks the earliest official release of a listing.
///
/// # Parameters
///
/// - `releases`: the release listing entries to choose from.
///
/// # Returns
///
/// The earliest release whose status is `Official`, or `None` when the listing
/// holds no official release.
pub fn earliest_official(
    releases: &[musicbrainz::ReleaseStub],
) -> Option<&musicbrainz::ReleaseStub> {
    earliest_of(releases.iter().filter(|release| is_official(release)))
}

/// Picks the first official release of a listing.
///
/// A recording listing orders a recording's releases by the web service's own
/// choice rather than by date, so the first official one is the release it
/// reports first.
///
/// # Parameters
///
/// - `releases`: the release listing entries to read, in order.
///
/// # Returns
///
/// The first release whose status is `Official`, or `None` when the listing
/// holds no official release.
pub fn first_official(releases: &[musicbrainz::ReleaseStub]) -> Option<&musicbrainz::ReleaseStub> {
    releases.iter().find(|release| is_official(release))
}

/// Picks the earliest of the releases an iterator yields.
///
/// # Parameters
///
/// - `releases`: the candidates.
///
/// # Returns
///
/// The candidate with the earliest date, or `None` when there is none.
fn earliest_of<'a>(
    releases: impl Iterator<Item = &'a musicbrainz::ReleaseStub>,
) -> Option<&'a musicbrainz::ReleaseStub> {
    releases.min_by_key(|release| date_key(release.date.as_deref()))
}

/// Builds the sort key of a release date.
///
/// # Parameters
///
/// - `date`: the release date, in the partial form the web service uses.
///
/// # Returns
///
/// A pair that orders partial ISO dates chronologically and pushes an absent or
/// empty date behind every real one.
pub fn date_key(date: Option<&str>) -> (bool, &str) {
    match date {
        Some(date) if !date.is_empty() => (false, date),
        _ => (true, ""),
    }
}

/// Returns one release per release group, keeping the earliest dated official
/// release of each.
///
/// The listing keeps the order in which the groups first appear, because that
/// is the order the web service ranked them in. A release without a release
/// group is its own entry, keyed by its release id.
///
/// # Parameters
///
/// - `releases`: the release listing entries to choose from.
///
/// # Returns
///
/// One entry per release group, in first-appearance order.
///
/// # Example
///
/// ```text
/// // a listing with two pressings of one group and one of another
/// official_by_release_group(&listing) -> [the 1998 pressing, the 2005 pressing]
/// ```
pub fn official_by_release_group(
    releases: &[musicbrainz::ReleaseStub],
) -> Vec<&musicbrainz::ReleaseStub> {
    let mut chosen: Vec<(&str, &musicbrainz::ReleaseStub)> = Vec::new();

    for release in releases.iter().filter(|release| is_official(release)) {
        let key = group_key(release);
        match chosen.iter_mut().find(|(seen, _)| *seen == key) {
            Some((_, kept)) => {
                if date_key(release.date.as_deref()) < date_key(kept.date.as_deref()) {
                    *kept = release;
                }
            }
            None => chosen.push((key, release)),
        }
    }

    chosen.into_iter().map(|(_, release)| release).collect()
}

/// Returns the key a release shares with the other pressings of its album.
///
/// # Parameters
///
/// - `release`: the release to key.
///
/// # Returns
///
/// The id of its release group, or its own id when the listing carried no
/// release group.
fn group_key(release: &musicbrainz::ReleaseStub) -> &str {
    release
        .release_group
        .as_ref()
        .and_then(|group| group.id.as_deref())
        .unwrap_or(&release.id)
}

/// What reading the browse pages of a release group found.
#[derive(Debug, Default)]
pub struct Browsed {
    /// The earliest release of the pages read.
    pub earliest: Option<musicbrainz::ReleaseStub>,
    /// Number of pages read.
    pub pages: u32,
    /// Number of releases the pages held.
    pub releases: usize,
    /// Whether [`EARLIEST_RELEASE_PAGES`] stopped the scan of a group that had
    /// more releases.
    pub capped: bool,
}

/// Reads the browse pages of a release group and picks the earliest release.
///
/// The web service does not order a browse by date, so the earliest release is
/// the minimum over the whole group rather than over the first page. The group
/// is read page by page until the page count the browse reports is reached, an
/// empty page arrives, or [`EARLIEST_RELEASE_PAGES`] pages were read; a group
/// past that cap is scanned on its first pages and the result reports that the
/// cap stopped the scan.
///
/// # Parameters
///
/// - `fetch`: reads the browse page at an offset into the group.
///
/// # Returns
///
/// What the pages held: the earliest release as [`earliest`] picks it, or
/// `None` when the group has no release.
///
/// # Errors
///
/// Returns the provider error of the page that failed.
pub fn earliest_browsed(
    mut fetch: impl FnMut(u32) -> Result<musicbrainz::ReleaseBrowse, ProviderError>,
) -> Result<Browsed, ProviderError> {
    let mut collected: Vec<musicbrainz::ReleaseStub> = Vec::new();
    let mut offset = 0;
    let mut pages = 0;
    let mut complete = false;

    while !complete && pages < EARLIEST_RELEASE_PAGES {
        let browse = fetch(offset)?;
        pages += 1;

        let returned = browse.releases.len() as u32;
        complete = match browse.release_count {
            Some(count) => collected.len() as u32 + returned >= count || returned == 0,
            None => true,
        };
        offset = browse.release_offset.unwrap_or(offset) + returned;
        collected.extend(browse.releases);
    }

    Ok(Browsed {
        earliest: earliest(&collected).cloned(),
        pages,
        releases: collected.len(),
        capped: !complete,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a release listing entry.
    fn release(id: &str, date: Option<&str>, status: Option<&str>) -> musicbrainz::ReleaseStub {
        musicbrainz::ReleaseStub {
            id: id.to_string(),
            title: "Album".to_string(),
            date: date.map(str::to_string),
            status: status.map(str::to_string),
            barcode: None,
            disambiguation: None,
            track_count: None,
            artist_credit: Vec::new(),
            media: Vec::new(),
            release_group: None,
        }
    }

    /// Builds an official release listing entry.
    fn official(id: &str, date: &str) -> musicbrainz::ReleaseStub {
        release(id, Some(date), Some("Official"))
    }

    /// Returns a copy of `release` in one release group.
    fn in_group(mut release: musicbrainz::ReleaseStub, group: &str) -> musicbrainz::ReleaseStub {
        release.release_group = Some(musicbrainz::ReleaseGroup {
            id: Some(group.to_string()),
            primary_type: Some("Album".to_string()),
            secondary_types: Vec::new(),
            genres: Vec::new(),
        });

        release
    }

    /// Builds a browse page of `ids`, all dated `date`.
    fn browse_page(
        ids: Vec<String>,
        date: &str,
        count: u32,
        offset: u32,
    ) -> musicbrainz::ReleaseBrowse {
        musicbrainz::ReleaseBrowse {
            releases: ids
                .iter()
                .map(|id| release(id, Some(date), Some("Official")))
                .collect(),
            release_count: Some(count),
            release_offset: Some(offset),
        }
    }

    /// Builds `count` release ids numbered from `prefix`.
    fn ids(prefix: &str, count: usize) -> Vec<String> {
        (0..count)
            .map(|index| format!("{prefix}-{index}"))
            .collect()
    }

    fn releases_of_a_group() -> Vec<musicbrainz::ReleaseStub> {
        let browse: musicbrainz::ReleaseBrowse =
            serde_json::from_str(include_str!("fixtures/releases_by_group.json")).unwrap();

        browse.releases
    }

    #[test]
    fn only_official_releases_qualify() {
        assert!(is_official(&official("x", "1998")));
        assert!(!is_official(&release("x", Some("1998"), Some("Promotion"))));
        assert!(!is_official(&release("x", Some("1998"), None)));
    }

    #[test]
    fn the_earliest_release_of_a_group_wins() {
        let releases = releases_of_a_group();
        let picked = earliest(&releases).unwrap();

        // The fixture's official releases start with 1998-09-22, 1998-04-20 and
        // 2002-02-13.
        assert_eq!(picked.id, "1a8207d9-75fa-44d7-9a83-04801b0619f3");
        assert_eq!(picked.date.as_deref(), Some("1998-04-20"));
        assert_eq!(
            earliest_official(&releases).map(|release| release.id.as_str()),
            Some("1a8207d9-75fa-44d7-9a83-04801b0619f3")
        );
    }

    #[test]
    fn only_the_official_releases_of_a_group_are_considered() {
        let releases = vec![
            release("promo", Some("1990"), Some("Promotion")),
            official("second", "1998-09-22"),
            official("first", "1998-04-20"),
        ];

        assert_eq!(
            earliest_official(&releases).map(|release| release.id.as_str()),
            Some("first")
        );
        assert_eq!(
            first_official(&releases).map(|release| release.id.as_str()),
            Some("second")
        );
        assert_eq!(
            earliest_official(&[release("promo", Some("1990"), Some("Promotion"))]).map(|r| &r.id),
            None
        );
    }

    #[test]
    fn a_listing_is_reduced_to_one_release_per_group() {
        let releases = vec![
            in_group(official("a-1998", "1998-04-20"), "a"),
            in_group(official("a-2005", "2005-03-01"), "a"),
            in_group(official("b-2002", "2002-02-13"), "b"),
            in_group(official("a-1996", "1996-01-01"), "a"),
        ];

        let chosen: Vec<&str> = official_by_release_group(&releases)
            .iter()
            .map(|release| release.id.as_str())
            .collect();

        // The group order is the order the groups first appear, and each group
        // is represented by its earliest release.
        assert_eq!(chosen, vec!["a-1996", "b-2002"]);
    }

    #[test]
    fn a_listing_without_groups_keeps_every_release() {
        let releases = vec![official("one", "1998"), official("two", "1998")];

        assert_eq!(official_by_release_group(&releases).len(), 2);
        assert!(official_by_release_group(&[]).is_empty());
    }

    #[test]
    fn an_undated_release_sorts_after_a_dated_one() {
        let page = [
            release("undated", None, Some("Official")),
            release("empty", Some(""), Some("Official")),
            official("dated", "1979"),
        ];

        assert_eq!(
            earliest(&page).map(|release| release.id.as_str()),
            Some("dated")
        );
        assert_eq!(earliest(&[]).map(|release| &release.id), None);
        assert_eq!(date_key(Some("")), (true, ""));
        assert_eq!(date_key(None), (true, ""));
        assert_eq!(date_key(Some("1998")), (false, "1998"));
    }

    #[test]
    fn a_group_that_fits_one_page_costs_one_request() {
        let mut requests = 0;

        let browsed = earliest_browsed(|offset| {
            requests += 1;
            Ok(browse_page(ids("release", 100), "2005-03-01", 100, offset))
        })
        .unwrap();

        assert_eq!(requests, 1);
        assert_eq!(browsed.pages, 1);
        assert_eq!(browsed.releases, 100);
        assert!(!browsed.capped);
        assert_eq!(
            browsed.earliest.as_ref().map(|release| release.id.as_str()),
            Some("release-0")
        );
    }

    #[test]
    fn the_earliest_release_is_picked_across_the_pages_of_a_group() {
        let mut requests = 0;

        // The first page holds releases from 2005 and the second a 1998 one
        // dated before them: a scan that stops after the first page dates the
        // group 2005.
        let browsed = earliest_browsed(|offset| {
            requests += 1;
            Ok(if offset == 0 {
                browse_page(ids("first", 100), "2005-03-01", 120, 0)
            } else {
                let mut second = browse_page(ids("second", 19), "2005-06-01", 120, 100);
                second.releases.push(official("earliest", "1998-04-20"));
                second
            })
        })
        .unwrap();

        assert_eq!(requests, 2);
        assert_eq!(browsed.releases, 120);
        assert!(!browsed.capped);
        assert_eq!(
            browsed.earliest.as_ref().map(|release| release.id.as_str()),
            Some("earliest")
        );
    }

    #[test]
    fn the_page_cap_stops_the_scan_of_a_group_with_more_releases() {
        let mut requests = 0;

        let browsed = earliest_browsed(|offset| {
            requests += 1;
            Ok(browse_page(
                ids(&format!("page-{offset}"), 100),
                "2005-03-01",
                1_000,
                offset,
            ))
        })
        .unwrap();

        assert_eq!(requests, EARLIEST_RELEASE_PAGES);
        assert_eq!(browsed.pages, EARLIEST_RELEASE_PAGES);
        assert_eq!(browsed.releases, 500);
        assert!(browsed.capped);
        assert_eq!(
            browsed.earliest.as_ref().map(|release| release.id.as_str()),
            Some("page-0-0")
        );
    }

    #[test]
    fn a_failed_page_fails_the_scan() {
        let browsed = earliest_browsed(|_| Err(ProviderError::Unavailable("HTTP 500".to_string())));

        assert!(matches!(browsed, Err(ProviderError::Unavailable(_))));
    }
}
