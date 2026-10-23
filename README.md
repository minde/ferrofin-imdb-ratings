# IMDb Ratings

A Ferrofin plugin that shows IMDb ratings in the web client:

- on movie and series cards, after the year: `2010 · IMDb 8.8`
- on movie and series detail pages, after the community and critic ratings

Hovering a badge shows the vote count. Titles with no IMDb id, or no IMDb
rating, show nothing.

## Where the ratings come from

IMDb's own [ratings dataset](https://developer.imdb.com/non-commercial-datasets/)
(`title.ratings.tsv.gz`, ~8.5 MB, refreshed daily; licensed for personal,
non-commercial use). No API key. The plugin's only network access is
`datasets.imdbws.com` (`egress` in `Cargo.toml`).

The IMDb id of each item is the one Ferrofin's metadata providers stored
(`ProviderIds.Imdb`). A wrongly matched title gets the wrong rating, or none;
fix it with Identify or Edit metadata.

## Install

1. Build and copy the `.wasm` as in the [top-level README](../README.md#build-and-install), restart Ferrofin.
2. The built-in **File Transformation** plugin must be enabled (it is by
   default): it applies the script tag the plugin adds to `index.html`.
3. Reload the web client. Apps that don't use jellyfin-web (Android TV,
   Swiftfin, Infuse…) don't get the badges.

No configuration. Disabling the plugin stops the ratings at once; the script
tag goes away after the next restart.

## How it works

- `src/client.js` (the browser side, injected into jellyfin-web): watches the
  page for Movie/Series cards and detail pages, gets their IMDb ids through
  the normal items API, asks the plugin for ratings in batches, and draws the
  badges.
- `src/lib.rs`: serves `GET /Plugins/{id}/web/imdb-ratings.js` (anonymous) and
  `GET /Plugins/{id}/web/ratings?imdb=tt…,tt…` (signed-in users, up to 200 ids).
  On a title it hasn't seen, or ratings older than a day, it downloads the
  dataset (at most once an hour) and keeps the 1.7M-row table in memory.
  Ratings it has served are cached in plugin state, so after a restart they
  show at once and nothing is downloaded until something is new or stale.
- `src/ratings.rs`: the dataset parser and lookup.
- Task **Refresh IMDb ratings** downloads now. It has no schedule by default
  and doesn't need one.

Measured on Ferrofin v1.3.2: download and parse 456 ms; cached answers return
immediately.

```bash
./test.sh    # parser and query tests on the host
./build.sh   # dist/imdb_ratings.wasm
```
