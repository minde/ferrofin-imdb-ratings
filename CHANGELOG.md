# Changelog

## 0.1.0

First release.

- IMDb rating after the year on movie and series cards, and next to the other
  ratings on movie and series pages, with the vote count on hover.
- Ratings from IMDb's daily dataset: downloaded on demand, refreshed when a
  day old, at most once an hour; ratings already shown survive restarts.
- Built against `ferrofin:plugin@0.5.0`; tested on Ferrofin v1.3.2 with
  jellyfin-web 12.0.0.
