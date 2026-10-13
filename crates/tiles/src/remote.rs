//! Fetches parts of files read in place by their web address (FlatGeobuf and GeoParquet, see
//! [`rustgis_data::source::remote`]) through the tile cache's fetchers: threads natively, the
//! browser's fetch on the web. Every address is checked first.

use rustgis_data::source::remote::{self, Want};

use crate::cache::{DOWNLOAD, RANGE, TAIL};

/// Installs the fetcher (once; later calls do nothing).
pub fn install() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        remote::set_fetcher(|url, want, done| {
            if let Err(e) = crate::address::check(url, None) {
                done(Err(e));
                return;
            }
            // A data download's size limit, not a tile's: a column chunk can be large.
            let marked = match want {
                Want::Range(a, b) if b > a => format!("{DOWNLOAD}{url}{RANGE}{a}-{}", b - 1),
                Want::Range(..) => {
                    done(Ok(Vec::new()));
                    return;
                }
                Want::Tail(n) => format!("{DOWNLOAD}{url}{TAIL}{n}"),
            };
            crate::cache::fetch_counted(marked, done);
        });
    });
}
