//! IMDb Ratings: a Ferrofin plugin that shows IMDb ratings in jellyfin-web.
//!
//! The server side downloads IMDb's daily ratings dataset and answers
//! `GET /ratings?imdb=tt…` for the browser. The browser side
//! (`client.js`, injected into index.html) finds movie and series cards and
//! detail pages, looks up their IMDb ids, and draws the badges.
//!
//! The full table lives only in memory. What the browser has asked for is
//! cached in plugin state, so after a restart ratings show at once and the
//! dataset is downloaded again only for a new title or once a day.

#[allow(clippy::too_many_arguments)]
mod bindings {
    wit_bindgen::generate!({
        path: "wit",
        world: "plugin",
    });
}

pub mod ratings;

use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use bindings::*;
use ferrofin::plugin::host;
use ferrofin::plugin::types::{HttpRequest, LogLevel};
use serde::{Deserialize, Serialize};

use ratings::{Rating, Table};

const DATASET_URL: &str = "https://datasets.imdbws.com/title.ratings.tsv.gz";
const REFRESH_TASK: &str = "refresh";
const CACHE_KEY: &str = "cache";
const DAY: u64 = 24 * 60 * 60;
/// After a failed or skipped download, wait this long before trying again,
/// so a broken network does not slow every page load.
const RETRY: u64 = 60 * 60;
const MAX_IDS_PER_REQUEST: usize = 200;
/// The host refuses state values over 1 MiB.
const MAX_CACHE_BYTES: usize = 900 * 1024;

/// Ratings the browser has asked for, as of `fetched_at`. `None` means the
/// title has no IMDb rating (asked again after the next download).
#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Cache {
    fetched_at: u64,
    ratings: HashMap<String, Option<(u16, u32)>>,
}

#[derive(Default)]
struct Runtime {
    cache: Cache,
    table: Option<Table>,
    table_at: u64,
    last_attempt: u64,
}

/// One instance lives for the server run; a crash rebuilds it, and the
/// cache comes back from state.
static RUNTIME: Mutex<Option<Runtime>> = Mutex::new(None);

struct Plugin;

impl Guest for Plugin {
    fn descriptor() -> PluginDescriptor {
        PluginDescriptor {
            id: env!("FERROFIN_PLUGIN_GUID").to_owned(),
            name: env!("FERROFIN_PLUGIN_NAME").to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            description: env!("FERROFIN_PLUGIN_DESCRIPTION").to_owned(),
        }
    }

    fn default_config() -> String {
        "{}".to_owned()
    }

    fn config_pages() -> Vec<ConfigPage> {
        Vec::new()
    }

    fn declared_egress() -> Vec<String> {
        env!("FERROFIN_PLUGIN_EGRESS")
            .split(',')
            .filter(|e| !e.is_empty())
            .map(str::to_owned)
            .collect()
    }

    fn provider_info() -> Option<ProviderDescriptor> {
        None
    }

    fn scan_targets() -> Vec<String> {
        Vec::new()
    }

    fn scan_media(_item: ItemSummary) -> Result<(), String> {
        Ok(())
    }

    /// Loads `client.js` on every jellyfin-web page. `/web/` is served as
    /// index.html, and `..` resolves to the server root from both.
    fn web_transforms() -> Vec<WebTransform> {
        vec![WebTransform {
            path_pattern: "index.html".to_owned(),
            search: "</body>".to_owned(),
            replace: format!(
                r#"<script defer src="../Plugins/{}/web/imdb-ratings.js"></script></body>"#,
                env!("FERROFIN_PLUGIN_GUID")
            ),
        }]
    }

    fn handle_request(request: PluginRequest) -> PluginResponse {
        match (request.method.as_str(), request.path.as_str()) {
            // Anonymous: loaded by a plain <script> tag, holds nothing private.
            ("GET", "/imdb-ratings.js") => response(
                200,
                "application/javascript",
                include_str!("client.js").replace("{{PLUGIN_ID}}", env!("FERROFIN_PLUGIN_GUID")).into_bytes(),
            ),
            // Signed-in users only: a miss can start a dataset download.
            ("GET", "/ratings") if !request.is_authenticated => {
                response(401, "application/json", br#"{"error":"sign in first"}"#.to_vec())
            }
            ("GET", "/ratings") => {
                let ids = imdb_ids(&request.query);
                match with_runtime(|rt| rt.lookup(&ids, now())) {
                    Some(found) => {
                        let body = serde_json::to_vec(&found).unwrap_or_else(|_| b"{}".to_vec());
                        response(200, "application/json", body)
                    }
                    None => response(503, "application/json", br#"{"error":"IMDb ratings not loaded yet"}"#.to_vec()),
                }
            }
            _ => response(404, "application/json", br#"{"error":"not found"}"#.to_vec()),
        }
    }

    fn tasks() -> Vec<TaskDescriptor> {
        vec![TaskDescriptor {
            id: REFRESH_TASK.to_owned(),
            name: "Refresh IMDb ratings".to_owned(),
            description: "Downloads IMDb's ratings dataset now. Without a schedule it is fetched when a title \
                          is missing or the ratings are a day old."
                .to_owned(),
            category: env!("FERROFIN_PLUGIN_NAME").to_owned(),
        }]
    }

    fn run_task(task_id: String) -> Result<(), String> {
        match task_id.as_str() {
            REFRESH_TASK => with_runtime(|rt| rt.download(now()).map(|()| rt.resolve_cached())),
            other => Err(format!("unknown task `{other}`")),
        }
    }

    fn on_event(_event_name: String, _event_json: String) {}

    fn remote_images(_item: ItemSummary) -> Result<Vec<ImageCandidate>, String> {
        Ok(vec![])
    }

    fn metadata_lookup(
        _item: ItemSummary,
        _provider_ids: Vec<(String, String)>,
    ) -> Result<Option<MetadataResult>, String> {
        Ok(None)
    }
}

impl Runtime {
    fn load() -> Self {
        let cache = host::get_state(CACHE_KEY)
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self { cache, ..Self::default() }
    }

    /// The ratings for `ids` that IMDb has, keyed by id. `None` when some id
    /// could not be checked (no dataset yet), so the browser asks again later
    /// instead of remembering "no rating".
    fn lookup(&mut self, ids: &[String], now: u64) -> Option<BTreeMap<String, Rating>> {
        let missing: Vec<&String> = ids.iter().filter(|id| !self.cache.ratings.contains_key(*id)).collect();
        let stale = now.saturating_sub(self.cache.fetched_at) >= DAY;
        if (stale || !missing.is_empty()) && self.wants_download(now) {
            if let Err(e) = self.download(now) {
                host::log(LogLevel::Warn, &format!("IMDb ratings download failed: {e}"));
            }
        }
        let Some(table) = &self.table else {
            return missing.is_empty().then(|| self.cached(ids));
        };
        for id in &missing {
            self.cache.ratings.insert((*id).clone(), table.get(id).map(pack));
        }
        if self.cache.fetched_at < self.table_at {
            self.resolve_cached();
        } else if !missing.is_empty() {
            self.save();
        }
        Some(self.cached(ids))
    }

    fn cached(&self, ids: &[String]) -> BTreeMap<String, Rating> {
        ids.iter()
            .filter_map(|id| {
                let (tenths, votes) = (*self.cache.ratings.get(id)?)?;
                Some((id.clone(), Rating { rating: f32::from(tenths) / 10.0, votes }))
            })
            .collect()
    }

    /// No fresh table in memory, and no attempt within the retry window.
    fn wants_download(&self, now: u64) -> bool {
        let fresh = self.table.is_some() && now.saturating_sub(self.table_at) < DAY;
        !fresh && now.saturating_sub(self.last_attempt) >= RETRY
    }

    fn download(&mut self, now: u64) -> Result<(), String> {
        self.last_attempt = now;
        let started = SystemTime::now();
        let response = host::http_fetch(&HttpRequest {
            method: "GET".to_owned(),
            url: DATASET_URL.to_owned(),
            headers: Vec::new(),
            body: None,
        })?;
        if response.status != 200 {
            return Err(format!("{DATASET_URL} answered HTTP {}", response.status));
        }
        let table = Table::parse(&response.body)?;
        host::log(
            LogLevel::Info,
            &format!(
                "IMDb ratings loaded: {} titles from {} KB in {} ms",
                table.len(),
                response.body.len() / 1024,
                started.elapsed().map_or(0, |d| d.as_millis())
            ),
        );
        self.table = Some(table);
        self.table_at = now;
        Ok(())
    }

    /// Re-reads every cached title from the in-memory table and saves.
    fn resolve_cached(&mut self) {
        let Some(table) = &self.table else { return };
        for (id, rating) in &mut self.cache.ratings {
            *rating = table.get(id).map(pack);
        }
        self.cache.fetched_at = self.table_at;
        self.save();
    }

    fn save(&self) {
        let Ok(bytes) = serde_json::to_vec(&self.cache) else { return };
        if bytes.len() > MAX_CACHE_BYTES {
            host::log(LogLevel::Warn, "IMDb ratings cache is full; not saving it");
            return;
        }
        if let Err(e) = host::set_state(CACHE_KEY, Some(&bytes)) {
            host::log(LogLevel::Warn, &format!("IMDb ratings cache not saved: {e}"));
        }
    }
}

fn with_runtime<T>(f: impl FnOnce(&mut Runtime) -> T) -> T {
    let mut guard = RUNTIME.lock().unwrap_or_else(|e| e.into_inner());
    f(guard.get_or_insert_with(Runtime::load))
}

fn pack(r: Rating) -> (u16, u32) {
    ((r.rating * 10.0).round() as u16, r.votes)
}

/// `imdb=tt1,tt2` from a raw query string; malformed ids are dropped.
fn imdb_ids(query: &str) -> Vec<String> {
    let Some(list) = query.split('&').find_map(|pair| pair.strip_prefix("imdb=")) else {
        return Vec::new();
    };
    list.replace("%2C", ",")
        .replace("%2c", ",")
        .split(',')
        .filter(|id| ratings::parse_id(id.as_bytes()).is_some())
        .take(MAX_IDS_PER_REQUEST)
        .map(str::to_owned)
        .collect()
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn response(status: u16, content_type: &str, body: Vec<u8>) -> PluginResponse {
    PluginResponse { status, headers: vec![("content-type".to_owned(), content_type.to_owned())], body }
}

bindings::export!(Plugin with_types_in bindings);

#[cfg(test)]
mod tests {
    use super::imdb_ids;

    #[test]
    fn ids_from_query_strings() {
        assert_eq!(imdb_ids("imdb=tt0133093,tt0903747"), ["tt0133093", "tt0903747"]);
        assert_eq!(imdb_ids("x=1&imdb=tt0133093%2Ctt0903747"), ["tt0133093", "tt0903747"]);
        assert_eq!(imdb_ids("imdb=tt0133093,../etc,tt,"), ["tt0133093"]);
        assert!(imdb_ids("").is_empty());
    }
}
