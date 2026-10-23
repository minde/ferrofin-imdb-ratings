// IMDb Ratings, browser side: injected into jellyfin-web's index.html.
// Adds "IMDb 7.8" after the year on movie and series cards, and next to the
// other ratings on their detail pages.
(function () {
  'use strict';
  var PLUGIN_ID = '{{PLUGIN_ID}}';
  var KINDS = { Movie: true, Series: true };
  var BATCH = 100;

  var imdbOf = {};    // item id -> IMDb id, or null for no id / not a movie or series
  var ratingOf = {};  // IMDb id -> { rating, votes }, or null for no rating
  var pending = {};   // ids being fetched
  var pausedUntil = 0;

  function api() {
    var client = window.ApiClient;
    return client && client.getCurrentUserId && client.getCurrentUserId() ? client : null;
  }

  // The text line that starts with the year: "1999", "2008 - 2013", "2019 - Present".
  // Older pages put the lines in a .cardFooter, the React library pages don't.
  function yearLine(card) {
    var lines = card.querySelectorAll('.cardText');
    for (var i = 0; i < lines.length; i++) {
      if (/^\s*\d{4}/.test(lines[i].textContent)) return lines[i];
    }
    return null;
  }

  function detailPage() {
    if (!/[#/]details\?/.test(location.hash + location.pathname + location.search)) return null;
    var match = /[?&]id=([0-9a-fA-F-]+)/.exec(location.hash) || /[?&]id=([0-9a-fA-F-]+)/.exec(location.search);
    var pages = document.querySelectorAll('.itemDetailPage:not(.hide)');
    var page = pages[pages.length - 1];
    var misc = page && page.querySelector('.itemMiscInfo-primary');
    return match && misc ? { id: match[1], misc: misc } : null;
  }

  // Everything on screen that needs a badge: [{ id, draw(rating) }].
  function targets() {
    var found = [];
    var cards = document.querySelectorAll('.card[data-id][data-type="Movie"], .card[data-id][data-type="Series"]');
    for (var i = 0; i < cards.length; i++) {
      var line = yearLine(cards[i]);
      if (line && !line.querySelector('.imdbRating')) {
        found.push({ id: cards[i].getAttribute('data-id'), draw: drawCard.bind(null, line) });
      }
    }
    var detail = detailPage();
    if (detail && !detail.misc.querySelector('.imdbRating')) {
      found.push({ id: detail.id, draw: drawDetail.bind(null, detail.misc) });
    }
    return found;
  }

  function badge(tag, className, r) {
    var el = document.createElement(tag);
    el.className = 'imdbRating ' + className;
    el.title = 'IMDb ' + r.rating.toFixed(1) + '/10 from ' + r.votes.toLocaleString() + ' votes';
    var logo = document.createElement('span');
    logo.className = 'imdbLogo';
    logo.textContent = 'IMDb';
    el.appendChild(logo);
    el.appendChild(document.createTextNode(r.rating.toFixed(1)));
    return el;
  }

  function drawCard(line, r) {
    var el = badge('span', 'imdbRating-card', r);
    el.insertBefore(document.createTextNode(' · '), el.firstChild);
    line.appendChild(el);
  }

  function drawDetail(misc, r) {
    var el = badge('div', 'mediaInfoItem imdbRating-detail', r);
    var others = misc.querySelectorAll('.starRatingContainer, .mediaInfoCriticRating');
    var anchor = others[others.length - 1];
    var endsAt = misc.querySelector('.endsAt');
    if (anchor) anchor.parentNode.insertBefore(el, anchor.nextSibling);
    else if (endsAt) endsAt.parentNode.insertBefore(el, endsAt);
    else misc.appendChild(el);
  }

  function draw() {
    targets().forEach(function (t) {
      var imdb = imdbOf[t.id];
      var r = imdb && ratingOf[imdb];
      if (r) t.draw(r);
    });
  }

  function chunks(list) {
    var out = [];
    for (var i = 0; i < list.length; i += BATCH) out.push(list.slice(i, i + BATCH));
    return out;
  }

  function fetchImdbIds(client, ids) {
    return Promise.all(chunks(ids).map(function (chunk) {
      return client.getItems(client.getCurrentUserId(), {
        Ids: chunk.join(','), Fields: 'ProviderIds', EnableImages: false, EnableUserData: false
      }).then(function (result) {
        chunk.forEach(function (id) { imdbOf[id] = null; });
        (result.Items || []).forEach(function (item) {
          var ids = item.ProviderIds || {};
          imdbOf[item.Id] = KINDS[item.Type] ? (ids.Imdb || ids.IMDB || null) : null;
        });
      });
    }));
  }

  function fetchRatings(client, imdbIds) {
    return Promise.all(chunks(imdbIds).map(function (chunk) {
      var url = client.getUrl('Plugins/' + PLUGIN_ID + '/web/ratings', { imdb: chunk.join(',') });
      return client.getJSON(url).then(function (result) {
        chunk.forEach(function (id) { ratingOf[id] = result[id] || null; });
      });
    }));
  }

  function update() {
    var client = api();
    if (!client || Date.now() < pausedUntil) return;
    var ids = targets().map(function (t) { return t.id; }).filter(function (id) {
      return !(id in imdbOf) && !pending[id];
    });
    ids = ids.filter(function (id, i) { return ids.indexOf(id) === i; });
    ids.forEach(function (id) { pending[id] = true; });

    fetchImdbIds(client, ids).then(function () {
      var wanted = targets().map(function (t) { return imdbOf[t.id]; }).filter(function (imdb, i, all) {
        return imdb && !(imdb in ratingOf) && !pending[imdb] && all.indexOf(imdb) === i;
      });
      wanted.forEach(function (imdb) { pending[imdb] = true; });
      return fetchRatings(client, wanted).finally(function () {
        wanted.forEach(function (imdb) { delete pending[imdb]; });
      });
    }).then(draw, function () {
      pausedUntil = Date.now() + 60000;
    }).finally(function () {
      ids.forEach(function (id) { delete pending[id]; });
    });
  }

  var timer = null;
  function schedule() {
    if (timer) return;
    timer = setTimeout(function () { timer = null; update(); }, 250);
  }

  var style = document.createElement('style');
  style.textContent =
    '.imdbRating{white-space:nowrap}' +
    '.imdbLogo{display:inline-block;background:#f5c518;color:#000;font:700 .75em/1.3 Arial,sans-serif;' +
    'border-radius:3px;padding:0 .3em;margin-right:.35em;vertical-align:.1em}';
  document.head.appendChild(style);

  new MutationObserver(schedule).observe(document.body, { childList: true, subtree: true });
  window.addEventListener('hashchange', schedule);
  schedule();
})();
