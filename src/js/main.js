import { api, onEvent } from './api.js';
import { countdownLineHtml, debounce, renderError, escapeAttr, skeletonGrid, skeletonSpotlight, startCountdownTicker } from './dom.js';
import { createSlideshow, seasonalCardHtml, recentEpisodeCardHtml, movieCardHtml } from './slideshow.js';
import { getContinueWatching, saveContinueWatching, removeContinueWatching, updateContinueWatchingProgress } from './continueWatching.js';
import { getWishlist, addToWishlist, removeFromWishlist, isInWishlist } from './wishlist.js';
import { addToWatched, removeFromWatched, isWatched } from './watched.js';
import { getNotifications, unreadCount, markAllRead, clearNotifications, recordEpisodeCounts } from './notifications.js';
import { getTmdbKey, setTmdbKey } from './tmdbKey.js';

let pendingPlay = null; // { kind: 'anime' | 'movie' | 'series', ...kind-specific fields }
let detailsOrigin = 'home'; // which page's back-button the Details page should return to

const SEASONS = ['winter', 'spring', 'summer', 'fall'];

// Slideshows keyed by the .slide-nav buttons' data-target, so one click
// handler (initSlideNav) can drive all of them.
const slideshows = {};

// ---------- View switching ----------

function showView(view) {
  document.querySelectorAll('.view').forEach((v) => v.classList.add('hidden'));
  const el = document.querySelector(`#view-${view}`);
  if (el) el.classList.remove('hidden');
  if (view === 'home') {
    renderContinueWatching();
    renderMyList();
    loadHeroContent();
    loadHomeSeasonal();
  }
}

function switchView(view) {
  document.querySelectorAll('.nav-item').forEach((b) => b.classList.remove('active'));
  const navBtn = document.querySelector(`.nav-item[data-view="${view}"]`);
  if (navBtn) navBtn.classList.add('active');
  showView(view);

  if (view === 'browse') loadBrowse();
  if (view === 'movies') loadMoviesTab();
  if (view === 'series') loadSeriesTab();
}

// ---------- Home: hero, continue watching, airing this season ----------

// Shared by the hero Resume button and the Continue Watching list — jumps
// straight to the Watching page for whatever kind of entry was saved.
// Missing `kind` (entries saved before movies/series were supported) is
// treated as 'anime' so old localStorage data still resumes correctly.
function resumeContinueWatching(entry) {
  detailsOrigin = 'home';
  const kind = entry.kind || 'anime';
  // Resuming from Home skips the kind's own Details page, so its container is
  // either empty or (worse) still showing whatever different title was last
  // browsed there this session — Back needs to point at Home instead of a
  // Details page that was never populated for this entry. anime/series get
  // this threaded through as backOverride so it survives every subsequent
  // Next/Prev Episode click too (each one re-runs select*ToWatch, which used
  // to silently reset Back to Details/Series Details); movies have no episode
  // nav to repeat the reset, so the one-time override below still covers them.
  const backOverride = { target: 'home', label: entry.title };
  if (kind === 'movie') {
    selectMovieToWatch({ title: entry.title, year: entry.year, image_url: entry.image_url }, entry.positionSeconds);
  } else if (kind === 'series') {
    selectSeriesEpisodeToWatch(
      { title: entry.title, year: entry.year, tmdb_id: entry.tmdb_id, image_url: entry.image_url },
      entry.season,
      entry.episode,
      undefined,
      backOverride,
      entry.positionSeconds
    );
  } else {
    selectEpisode({ title: entry.title, status: entry.status, image_url: entry.image_url }, entry.episode, entry.quality, undefined, backOverride);
  }
  // Safe to call after the (possibly async) select*ToWatch call above: each
  // one calls enterWatching — which sets the back target — synchronously
  // before its first await, so this always runs after and wins. Redundant
  // for anime/series now that backOverride covers them, but still the only
  // thing doing it for movies.
  setWatchingBackTarget('home', entry.title);
}

function continueWatchingSubtitle(entry) {
  const kind = entry.kind || 'anime';
  if (kind === 'movie') return 'Movie';
  if (kind === 'series') return `Season ${entry.season} · Episode ${entry.episode}`;
  return `Episode ${entry.episode}`;
}

// Renders a hero-meta line as dot-separated pieces (".hero-meta .dot-sep" in
// styles.css) instead of one "a · b · c" text node, so each piece is its own
// flex item and the separator can be styled/spaced independently of the text.
function renderMetaParts(elId, parts) {
  document.getElementById(elId).innerHTML = parts
    .filter(Boolean)
    .map((p) => `<span>${escapeAttr(p)}</span>`)
    .join('<span class="dot-sep"></span>');
}

// The hero shows whatever's most relevant: the user's most recent
// continue-watching entry if there is one (with a real Resume CTA), else the
// #1 trending anime (with a Details CTA) — never a mismatched combination of
// the two.
async function renderHeroResume(entry) {
  const kind = entry.kind || 'anime';
  document.getElementById('hero-backdrop').style.backgroundImage = entry.image_url ? `url("${entry.image_url}")` : '';
  document.getElementById('hero-tag-label').textContent = 'Continue where you left off';
  document.getElementById('hero-title').textContent = entry.title;
  renderMetaParts('hero-meta', [continueWatchingSubtitle(entry)]);
  document.getElementById('hero-desc').textContent = '';
  const resumeBtn = document.getElementById('hero-resume-btn');
  resumeBtn.classList.remove('hidden');
  resumeBtn.onclick = () => resumeContinueWatching(entry);
  const detailsBtn = document.getElementById('hero-details-btn');
  // ponytail: movie/series entries only carry title/year (see startPlayback),
  // not the tmdb_id/genres/overview selectMovie/selectSeries need to render a
  // real Details page — hide Details rather than build it from partial data.
  // Ceiling: store tmdb_id in pendingPlay if a movie/series Details CTA from
  // here is ever requested.
  detailsBtn.classList.toggle('hidden', kind !== 'anime');
  if (kind === 'anime') {
    detailsBtn.onclick = () => {
      detailsOrigin = 'home';
      selectAnime({ title: entry.title });
    };
  }
}

function renderHeroTrending(anime) {
  document.getElementById('hero-backdrop').style.backgroundImage = anime.image_url ? `url("${anime.image_url}")` : '';
  document.getElementById('hero-tag-label').textContent = 'Trending now';
  document.getElementById('hero-title').textContent = anime.title;
  const metaParts = [
    anime.episodes ? `${anime.episodes} episodes` : capitalize(anime.status || ''),
    anime.score ? `★ ${anime.score}` : '',
    (anime.genres || []).slice(0, 3).join(', '),
  ].filter(Boolean);
  renderMetaParts('hero-meta', metaParts);
  document.getElementById('hero-desc').textContent = anime.synopsis || '';
  document.getElementById('hero-resume-btn').classList.add('hidden');
  const detailsBtn = document.getElementById('hero-details-btn');
  detailsBtn.classList.remove('hidden');
  detailsBtn.onclick = () => {
    detailsOrigin = 'home';
    selectAnime(anime);
  };
}

let cachedTopTrending = null;

async function loadHeroContent() {
  const entries = getContinueWatching();
  if (entries.length) {
    renderHeroResume(entries[0]);
    return;
  }
  if (cachedTopTrending) {
    renderHeroTrending(cachedTopTrending);
    return;
  }
  try {
    const trending = await api.getTrendingAnime();
    if (!trending.length) return;
    cachedTopTrending = trending[0];
    renderHeroTrending(cachedTopTrending);
  } catch {
    // Gradient fallback in CSS covers this — nothing else to do.
  }
}

function renderContinueWatching() {
  const section = document.getElementById('continue-watching-section');
  const list = document.getElementById('continue-watching-list');
  const entries = getContinueWatching();

  if (!entries.length) {
    section.classList.add('hidden');
    return;
  }
  section.classList.remove('hidden');

  // Reuses .poster-card/.poster-image-wrap/.poster-reveal (see slideshow.js)
  // now that saved entries carry image_url — old entries saved before that
  // (no image_url) just fall back to the poster-gradient placeholder.
  list.innerHTML = entries
    .map((e, i) => {
      // Only rendered when both numbers are real — a missing duration (mpv
      // couldn't be IPC-queried, or the value hasn't been reported at all
      // yet) means no bar rather than a wrong/nonsensical one.
      const hasProgress = e.durationSeconds > 0 && e.positionSeconds != null;
      const progressBar = hasProgress
        ? `<div class="continue-progress"><div class="continue-progress-fill" style="width:${Math.min(100, (e.positionSeconds / e.durationSeconds) * 100).toFixed(1)}%"></div></div>`
        : '';
      return `
      <div class="poster-card" data-index="${i}" tabindex="0" role="button" aria-label="Resume ${escapeAttr(e.title)}">
        <div class="poster-image-wrap">
          ${e.image_url ? `<img src="${e.image_url}" alt="${escapeAttr(e.title)}" loading="lazy" />` : ''}
          <button class="continue-remove" data-remove="${i}" aria-label="Remove" type="button">&times;</button>
          ${progressBar}
          <div class="poster-reveal">
            <div class="poster-title">${e.title}</div>
            <div class="poster-sub">${continueWatchingSubtitle(e)}</div>
          </div>
        </div>
      </div>
    `;
    })
    .join('') +
    `<button type="button" id="continue-find-new" class="see-all-card continue-find-new">
      <span class="continue-find-icon">+</span>
      <span>Find something new</span>
    </button>`;

  list.querySelectorAll('.continue-remove').forEach((btn) => {
    btn.addEventListener('click', (ev) => {
      ev.stopPropagation();
      removeContinueWatching(entries[Number(btn.dataset.remove)].id);
      renderContinueWatching();
      loadHeroContent();
    });
  });

  list.querySelectorAll('.poster-card[data-index]').forEach((item) => {
    item.addEventListener('click', () => {
      resumeContinueWatching(entries[Number(item.dataset.index)]);
    });
  });

  document.getElementById('continue-find-new').addEventListener('click', () => switchView('browse'));
}

// Reuses the same poster-card renderers Browse/Movies/Series already use —
// a wishlist entry is a subset of what those cards need (title/image_url/
// year), so no new card markup.
function renderMyList() {
  const section = document.getElementById('wishlist-section');
  const grid = document.getElementById('wishlist-grid');
  const entries = getWishlist();

  if (!entries.length) {
    section.classList.add('hidden');
    return;
  }
  section.classList.remove('hidden');
  grid.innerHTML = entries.map((e, i) => `<div class="poster-card" data-index="${i}" tabindex="0" role="button">${e.kind === 'anime' ? seasonalCardHtml(e) : movieCardHtml(e)}</div>`).join('');
  grid.querySelectorAll('.poster-card').forEach((card) => {
    card.addEventListener('click', () => {
      const entry = entries[Number(card.dataset.index)];
      detailsOrigin = 'home';
      if (entry.kind === 'movie') selectMovie(entry);
      else if (entry.kind === 'series') selectSeries(entry);
      else selectAnime(entry);
    });
  });
}

// Shared by the anime/movie/series wishlist buttons — toggles membership and
// keeps the Home "My List" section (if currently visible) in sync.
function wireWishlistButton(btnId, entry) {
  const btn = document.getElementById(btnId);
  const sync = () => {
    btn.textContent = isInWishlist(entry.id) ? '✓ In Wishlist' : '+ Wishlist';
  };
  sync();
  btn.onclick = () => {
    if (isInWishlist(entry.id)) removeFromWishlist(entry.id);
    else addToWishlist(entry);
    sync();
    renderMyList();
  };
}

// Same toggle pattern as wireWishlistButton, but for a plain "I've seen
// this" flag — no list/page of its own for v1, just visible on the title's
// own Details page so revisiting it shows whether you've already watched it.
function wireWatchedButton(btnId, entry) {
  const btn = document.getElementById(btnId);
  const sync = () => {
    btn.textContent = isWatched(entry.id) ? '✓ Watched' : 'Mark as Watched';
  };
  sync();
  btn.onclick = () => {
    if (isWatched(entry.id)) removeFromWatched(entry.id);
    else addToWatched(entry);
    sync();
  };
}

// ---------- Notifications: new-episode check for tracked anime/series ----------

function renderNotifBadge() {
  const badge = document.getElementById('notif-badge');
  const count = unreadCount();
  badge.textContent = count > 9 ? '9+' : String(count);
  badge.classList.toggle('hidden', count === 0);
}

// Notifications only store id/kind/title/message — full metadata (image_url,
// tmdb_id, season/episode) for jumping to Details lives on whichever
// Continue Watching / Wishlist entry the notification came from.
function findTrackedEntry(id) {
  return [...getContinueWatching(), ...getWishlist()].find((e) => e.id === id);
}

function renderNotifList() {
  const list = document.getElementById('notif-list');
  const entries = getNotifications();
  if (!entries.length) {
    list.innerHTML = '<div class="notif-empty">No notifications yet</div>';
    return;
  }
  list.innerHTML = entries
    .map(
      (n, i) => `
      <div class="notif-item${n.read ? '' : ' unread'}" data-index="${i}" tabindex="0" role="button" aria-label="${escapeAttr(n.title)}">
        <div class="notif-item-title">${n.title}</div>
        <div class="notif-item-msg">${n.message}</div>
      </div>
    `
    )
    .join('');
  list.querySelectorAll('.notif-item').forEach((el) => {
    el.addEventListener('click', () => {
      const entry = findTrackedEntry(entries[Number(el.dataset.index)].itemId);
      document.getElementById('notif-dropdown').classList.add('hidden');
      if (!entry) return;
      detailsOrigin = 'home';
      if (entry.kind === 'movie') selectMovie(entry);
      else if (entry.kind === 'series') selectSeries(entry);
      else selectAnime(entry);
    });
  });
}

function initNotifications() {
  const bell = document.getElementById('notif-bell-btn');
  const dropdown = document.getElementById('notif-dropdown');
  bell.addEventListener('click', (e) => {
    e.stopPropagation();
    if (!dropdown.classList.contains('hidden')) {
      dropdown.classList.add('hidden');
      bell.setAttribute('aria-expanded', 'false');
      return;
    }
    renderNotifList();
    dropdown.classList.remove('hidden');
    bell.setAttribute('aria-expanded', 'true');
    markAllRead();
    renderNotifBadge();
  });
  document.getElementById('notif-clear-btn').addEventListener('click', (e) => {
    e.stopPropagation();
    clearNotifications();
    renderNotifList();
    renderNotifBadge();
  });
  document.addEventListener('click', (e) => {
    if (!e.target.closest('.nav-notif-wrap')) {
      dropdown.classList.add('hidden');
      bell.setAttribute('aria-expanded', 'false');
    }
  });
}

// Only anime/series get "new episode" checks — movies don't get new
// episodes after release. Runs once at startup and on an interval while the
// app stays open; a closed app just catches up on the next launch, same as
// most desktop apps without a background service.
async function checkNotifications() {
  const tracked = new Map();
  for (const e of [...getContinueWatching(), ...getWishlist()]) {
    if (e.kind === 'anime' || e.kind === 'series') tracked.set(e.id, e);
  }
  // Movies only make sense from Wishlist (an in-progress/finished continue-
  // watching movie is obviously already out) and don't need a re-fetch the
  // way episode counts do — a release date doesn't move, only whether
  // "today" has passed it does, so this is pure date math against whatever
  // was already stored when it was wishlisted.
  const trackedMovies = getWishlist().filter((e) => e.kind === 'movie');
  if (!tracked.size && !trackedMovies.length) return;

  const episodeItems = await Promise.all(
    [...tracked.values()].map(async (e) => {
      const base = { id: e.id, kind: e.kind, title: e.title };
      try {
        if (e.kind === 'anime') {
          const info = await api.getAnimeInfo(e.title);
          return { ...base, count: info?.episodes ?? null };
        }
        const key = getTmdbKey();
        if (!key || !e.tmdb_id) return { ...base, count: null };
        const seasons = await api.getTvSeasons(e.tmdb_id, key);
        return { ...base, count: seasons.reduce((sum, s) => sum + s.episode_count, 0) };
      } catch {
        return { ...base, count: null };
      }
    })
  );
  const movieItems = trackedMovies.map((e) => ({
    id: e.id,
    kind: 'movie',
    title: e.title,
    count: e.release_date ? (new Date(e.release_date) <= new Date() ? 1 : 0) : null,
  }));

  recordEpisodeCounts([...episodeItems, ...movieItems]);
  renderNotifBadge();
}

let homeSeasonalLoaded = false;

async function loadHomeSeasonal() {
  if (homeSeasonalLoaded) return;
  homeSeasonalLoaded = true;
  const el = document.getElementById('home-seasonal-grid');
  try {
    const items = (await api.getSeasonalAnime()).slice(0, 12);
    el.innerHTML = items.map((item, i) => `<div class="poster-card" data-index="${i}" tabindex="0" role="button">${seasonalCardHtml(item)}</div>`).join('');
    el.querySelectorAll('.poster-card').forEach((card) => {
      card.addEventListener('click', () => {
        detailsOrigin = 'home';
        selectAnime(items[Number(card.dataset.index)]);
      });
    });
  } catch (e) {
    homeSeasonalLoaded = false;
    renderError(el, `Error loading seasonal anime: ${e}`, loadHomeSeasonal);
  }
}

// ---------- Browse: season picker, slideshows, trending ----------

function capitalize(s) {
  return s.charAt(0).toUpperCase() + s.slice(1);
}

function currentSeasonInfo(date = new Date()) {
  return { year: date.getFullYear(), season: SEASONS[Math.floor(date.getMonth() / 3)] };
}

function seasonKey(year, season) {
  return year * 4 + SEASONS.indexOf(season);
}

// Flattens get_season_list's year-descending, seasons-ascending list into a
// most-recent-first sequence, dropping the current season and anything
// not-yet-aired so only real past seasons remain.
function flattenPastSeasons(seasonYears, current, count) {
  const currentKey = seasonKey(current.year, current.season);
  const flat = [];
  for (const y of seasonYears) {
    for (let i = SEASONS.length - 1; i >= 0; i--) {
      const season = SEASONS[i];
      if (y.seasons.includes(season) && seasonKey(y.year, season) < currentKey) {
        flat.push({ year: y.year, season });
      }
    }
  }
  return flat.slice(0, count);
}

async function renderSeasonButtons() {
  const el = document.getElementById('season-buttons');
  el.innerHTML = '<button class="season-btn active" data-current="1">Current Season</button>';

  try {
    const seasonYears = await api.getSeasonList();
    const pastSeasons = flattenPastSeasons(seasonYears, currentSeasonInfo(), 7);
    el.innerHTML += pastSeasons
      .map((s) => `<button class="season-btn" data-year="${s.year}" data-season="${s.season}">${capitalize(s.season)} ${s.year}</button>`)
      .join('');
  } catch (e) {
    console.error('get_season_list error', e);
  }

  el.querySelectorAll('.season-btn').forEach((btn) => {
    btn.addEventListener('click', () => {
      el.querySelectorAll('.season-btn').forEach((b) => b.classList.remove('active'));
      btn.classList.add('active');
      if (btn.dataset.current) {
        loadSeasonal(null, null, 'This Season');
      } else {
        const label = `${capitalize(btn.dataset.season)} ${btn.dataset.year}`;
        loadSeasonal(Number(btn.dataset.year), btn.dataset.season, label);
      }
    });
  });
}

// Remembers the last season picked so the genre filter (a separate control)
// can re-run the same season query when it changes.
let currentSeasonSelection = { year: null, season: null, label: null };

async function loadSeasonal(year, season, label) {
  currentSeasonSelection = { year, season, label };
  document.getElementById('seasonal-heading').textContent = label || 'This Season';
  slideshows.seasonal.loading();
  try {
    const genre = document.getElementById('anime-genre-filter').value || undefined;
    const items = year && season ? await api.getSeasonalAnimeBy(year, season, genre) : await api.getSeasonalAnime(genre);
    slideshows.seasonal.setItems(items, seasonalCardHtml);
  } catch (e) {
    renderError(document.getElementById('seasonal-track'), `Error loading anime: ${e}`, () => loadSeasonal(year, season, label));
  }
}

async function loadRecentEpisodes() {
  slideshows.recent.loading();
  try {
    const items = await api.getRecentEpisodes();
    slideshows.recent.setItems(items, recentEpisodeCardHtml);
  } catch (e) {
    renderError(document.getElementById('recent-track'), `Error loading new episodes: ${e}`, loadRecentEpisodes);
  }
}

async function loadTrendingPanel() {
  const el = document.getElementById('trending-panel');
  try {
    el.innerHTML = '<p class="muted-center">Loading...</p>';
    const items = await api.getTrendingAnime();
    if (!items.length) {
      el.innerHTML = '<p class="muted-center">No trending data</p>';
      return;
    }
    el.innerHTML = items
      .map(
        (a, i) => `
        <div class="trending-item" data-index="${i}" tabindex="0" role="button" aria-label="${escapeAttr(a.title)}">
          <div class="trending-rank">${i + 1}</div>
          <img src="${a.image_url}" alt="${escapeAttr(a.title)}" loading="lazy" />
          <div class="trending-info">
            <div class="trending-title">${a.title}</div>
            ${a.score ? `<div class="trending-sub">★ ${a.score}</div>` : ''}
          </div>
        </div>
      `
      )
      .join('');
    el.querySelectorAll('.trending-item').forEach((row) => {
      row.addEventListener('click', () => {
        detailsOrigin = 'browse';
        selectAnime(items[Number(row.dataset.index)]);
      });
    });
  } catch (e) {
    renderError(el, `Error loading trending: ${e}`, loadTrendingPanel);
  }
}

// A short stagger between the four Browse requests keeps things feeling
// instant while staying well under AniList's rate limit.
function loadBrowse() {
  loadSeasonal();
  setTimeout(loadRecentEpisodes, 200);
  setTimeout(loadTrendingPanel, 400);
  setTimeout(renderSeasonButtons, 600);
}

// ---------- Spotlight: random-pick card (Browse/Movies/Series) ----------

let spotlightLoaded = false;

// Shared by the anime/movie/series spotlight cards — same backdrop hero used
// on Home (containerEl carries class="hero" in index.html now, not
// "spotlight-card"), just with a Shuffle action alongside Details. h2 (not
// h1) because each of these views already has its own <h1 class="page-title">.
function renderSpotlightCard(containerEl, item, subParts, { onDetails, onShuffle }) {
  const metaParts = [...subParts, (item.genres || []).slice(0, 3).join(', ')].filter(Boolean);
  containerEl.innerHTML = `
    <div class="hero-backdrop" style="${item.image_url ? `background-image:url('${item.image_url}')` : ''}"></div>
    <div class="hero-overlay"></div>
    <div class="hero-content">
      <span class="tag">Random pick</span>
      <h2>${item.title}</h2>
      <p class="hero-meta">${metaParts.map((p) => `<span>${escapeAttr(p)}</span>`).join('<span class="dot-sep"></span>')}</p>
      <p class="hero-desc">${item.synopsis || item.overview || ''}</p>
      <div class="hero-cta">
        <button type="button" class="btn btn-primary" data-action="details">Details</button>
        <button type="button" class="btn btn-ghost" data-action="shuffle">&#8635; Shuffle</button>
      </div>
    </div>
  `;
  containerEl.querySelector('[data-action="details"]').addEventListener('click', onDetails);
  containerEl.querySelector('[data-action="shuffle"]').addEventListener('click', onShuffle);
}

// Draws from AniList's whole popularity-ranked catalog, not just the current
// season like the seasonal charts elsewhere on this page.
async function loadAnimeSpotlight() {
  const el = document.getElementById('spotlight-card');
  el.innerHTML = skeletonSpotlight();
  try {
    const item = await api.getRandomAnime();
    if (!item) {
      el.innerHTML = '<p class="muted-center">No data</p>';
      return;
    }
    const sub = [item.episodes ? `${item.episodes} episodes` : capitalize(item.status || ''), item.score ? `★ ${item.score}` : ''].filter(Boolean);
    renderSpotlightCard(el, item, sub, {
      onDetails: () => {
        detailsOrigin = 'browse';
        selectAnime(item);
      },
      onShuffle: loadAnimeSpotlight,
    });
  } catch (e) {
    renderError(el, `Error loading spotlight: ${e}`, loadAnimeSpotlight);
  }
}

async function loadMediaSpotlight(mediaType, containerId, onSelect) {
  const el = document.getElementById(containerId);
  el.innerHTML = skeletonSpotlight();
  try {
    const item = await api.getRandomMedia(mediaType, undefined, getTmdbKey());
    if (!item) {
      el.innerHTML = '<p class="muted-center">No data</p>';
      return;
    }
    renderSpotlightCard(el, item, movieSubParts(item), {
      onDetails: () => onSelect(item),
      onShuffle: () => loadMediaSpotlight(mediaType, containerId, onSelect),
    });
  } catch (e) {
    renderError(el, `Error loading spotlight: ${e}`, () => loadMediaSpotlight(mediaType, containerId, onSelect));
  }
}

async function loadRecentScroll() {
  const scrollEl = document.getElementById('browse-recent-scroll');
  scrollEl.innerHTML = skeletonGrid(5);
  try {
    const items = await api.getRecentEpisodes();
    scrollEl.innerHTML =
      items.map((item, i) => `<div class="poster-card" data-index="${i}" tabindex="0" role="button">${recentEpisodeCardHtml(item)}</div>`).join('') ||
      '<p class="muted-center">No recent episodes</p>';
    scrollEl.querySelectorAll('.poster-card').forEach((card) => {
      card.addEventListener('click', () => {
        detailsOrigin = 'browse';
        selectAnime({ title: items[Number(card.dataset.index)].title });
      });
    });
  } catch (e) {
    renderError(scrollEl, `Error loading recent episodes: ${e}`, loadRecentScroll);
  }
}

function loadSpotlight() {
  if (spotlightLoaded) return;
  spotlightLoaded = true;
  loadAnimeSpotlight();
  loadRecentScroll();
}

function initBrowseLayoutToggle() {
  document.querySelectorAll('#browse-layout-seg input[name="browse-layout"]').forEach((input) => {
    input.addEventListener('change', () => {
      const mode = input.value;
      document.getElementById('browse-spotlight').classList.toggle('hidden', mode !== 'spotlight');
      document.getElementById('browse-seasons').classList.toggle('hidden', mode !== 'seasons');
      if (mode === 'spotlight') loadSpotlight();
    });
  });
}

async function initAnimeGenreFilter() {
  const select = document.getElementById('anime-genre-filter');
  select.addEventListener('change', () => {
    const { year, season, label } = currentSeasonSelection;
    loadSeasonal(year, season, label);
  });
  try {
    const genres = await api.getAnimeGenres();
    select.innerHTML = '<option value="">All genres</option>' + genres.map((g) => `<option value="${escapeAttr(g)}">${escapeAttr(g)}</option>`).join('');
  } catch (e) {
    console.error('get_anime_genres error', e);
  }
}

// AniList's own search can miss on a longer/compound title (e.g. "Attack on
// Titan: The Final Season"); retrying with just the part before the first
// colon or parenthesis cuts down "no info found" for the get_anime_info
// enrichment lookup in selectAnime below.
function simplifyTitle(title) {
  return title.split(/[:(]/)[0].trim();
}

// ---------- Details & episodes ----------

function setDetailsBackdrop(elId, imageUrl) {
  const el = document.getElementById(elId);
  if (el) el.style.backgroundImage = imageUrl ? `url("${imageUrl}")` : '';
}

// Episode buttons are just 1..count generated locally — there's no fetch, and
// no id, since ani-cli owns search/episode resolution entirely now (see
// watch_episode). `anime` only needs a `title` for that to work later.
function renderEpisodeGrid(anime, count) {
  if (!count) return;
  document.getElementById('episodes-card').classList.remove('hidden');
  const numbers = Array.from({ length: count }, (_, i) => String(i + 1));
  const html = numbers.map((n) => `<button class="episode-btn" data-episode="${n}">${n}</button>`).join('');
  document.getElementById('episodes-list').innerHTML = html;

  document.querySelectorAll('.episode-btn').forEach((btn) => {
    btn.addEventListener('click', function () {
      document.querySelectorAll('.episode-btn').forEach((b) => b.classList.remove('active'));
      this.classList.add('active');
      selectEpisode(anime, this.dataset.episode, undefined, numbers);
    });
  });
}

// Caches AniList enrichment lookups for shallow { title }-only entries
// (Continue Watching, Recent Episodes) so reopening the same title doesn't
// re-hit AniList every time — a plain title->info map, cleared on reload.
const animeInfoCache = new Map();

// `anime` is either a full AniList record (Browse/Home/Trending/Spotlight/
// palette results already carry synopsis/genres/episodes) or just a bare
// { title } (Continue Watching, Recent Episodes) — the latter gets enriched
// via a single AniList lookup. Either way, ani-cli itself resolves and plays
// whatever title ends up in pendingPlay; this is display metadata only.
async function selectAnime(anime) {
  showView('details');
  setDetailsBackdrop('details-backdrop', anime.image_url || null);
  const backBtn = document.querySelector('#view-details .back-btn');
  if (backBtn) {
    backBtn.dataset.back = detailsOrigin;
    backBtn.innerHTML = detailsOrigin === 'browse' ? '&larr; Back to Browse' : '&larr; Back to Home';
  }

  const container = document.getElementById('anime-details-container');
  const hasMetadata = anime.genres !== undefined;
  if (hasMetadata) {
    const sub = [anime.episodes ? `${anime.episodes} episodes` : capitalize(anime.status || ''), anime.score ? `★ ${anime.score}` : ''].filter(Boolean);
    container.innerHTML = mediaDetailsHtml(anime, sub, anime.synopsis);
  } else {
    container.innerHTML = `<h3>${anime.title}</h3>`;
  }
  renderEpisodeGrid(anime, anime.episodes);
  wireWishlistButton('anime-wishlist-btn', { id: anime.title, kind: 'anime', title: anime.title, image_url: anime.image_url });
  // Unlike wishlist, watched doesn't need image_url (no list view for it
  // yet), so id/kind/title are enough and it doesn't need re-wiring in the
  // enrichment branch below — none of those change between the two paths.
  wireWatchedButton('anime-watched-btn', { id: anime.title, kind: 'anime', title: anime.title });

  if (hasMetadata) {
    loadAnimeRecommendations(anime.id);
    return;
  }
  try {
    let info = animeInfoCache.get(anime.title);
    if (info === undefined) {
      info = await api.getAnimeInfo(anime.title);
      if (!info) {
        const simplified = simplifyTitle(anime.title);
        if (simplified && simplified !== anime.title) {
          info = await api.getAnimeInfo(simplified);
        }
      }
      animeInfoCache.set(anime.title, info || null);
    }
    if (info) {
      const sub = [info.episodes ? `${info.episodes} episodes` : capitalize(info.status || ''), info.score ? `★ ${info.score}` : ''].filter(Boolean);
      container.innerHTML = mediaDetailsHtml({ ...info, title: anime.title }, sub, info.synopsis);
      setDetailsBackdrop('details-backdrop', info.image_url);
      // Only `status` is copied onto the shared `anime` object (used later by
      // startPlayback to pick the resolve_anime disambiguation strategy) —
      // NOT `episodes`, since for these shallow entry points episodeNum is
      // already a real (not local 1..N) episode number, and setting
      // anime.episodes would wrongly trigger the local->real remap below.
      anime.status = info.status;
      renderEpisodeGrid(anime, info.episodes);
      wireWishlistButton('anime-wishlist-btn', { id: anime.title, kind: 'anime', title: anime.title, image_url: info.image_url });
      loadAnimeRecommendations(info.id);
    }
  } catch (e) {
    console.error('get_anime_info error', e); // enrichment is best-effort — bare title above already covers this
  }
}

// "Because you watched X" — detailsOrigin is deliberately left untouched so
// clicking through to another title's Details still returns to wherever the
// *original* title was opened from, not back to this in-between page.
async function loadAnimeRecommendations(id) {
  const key = 'animeRecs';
  if (!slideshows[key]) {
    slideshows[key] = createSlideshow(document.getElementById('anime-recs-track'), { onSelect: selectAnime });
  }
  slideshows[key].loading();
  try {
    const items = await api.getAnimeRecommendations(id);
    slideshows[key].setItems(items, seasonalCardHtml);
  } catch (e) {
    renderError(document.getElementById('anime-recs-track'), `Error loading recommendations: ${e}`, () => loadAnimeRecommendations(id));
  }
}

async function loadMediaRecommendations(mediaType, tmdbId, trackId, slideshowKey, onSelect) {
  if (!slideshows[slideshowKey]) {
    slideshows[slideshowKey] = createSlideshow(document.getElementById(trackId), { onSelect });
  }
  slideshows[slideshowKey].loading();
  try {
    const items = await api.getRecommendations(mediaType, tmdbId, getTmdbKey());
    slideshows[slideshowKey].setItems(items, movieCardHtml);
  } catch (e) {
    renderError(document.getElementById(trackId), `Error loading recommendations: ${e}`, () =>
      loadMediaRecommendations(mediaType, tmdbId, trackId, slideshowKey, onSelect)
    );
  }
}

// ---------- Movies & Series ----------

let moviesLoaded = false;
let seriesLoaded = false;

// Search and genre/year discovery share one results grid per page (whichever
// the user touched most recently wins) and both page through the *real*
// result count from TMDB via a "Load more" button, instead of silently
// capping at whatever the first page contained. State is keyed by the grid's
// element id since search and discover are mutually exclusive for a given grid.
const gridState = new Map(); // resultsId -> { items, page, hasMore, onSelect, fetchPage }

function renderMediaGrid(resultsId) {
  const state = gridState.get(resultsId);
  const el = document.getElementById(resultsId);
  if (!state.items.length) {
    el.innerHTML = '<p class="muted-center">No results found</p>';
    return;
  }
  const cards = state.items.map((item, i) => `<div class="poster-card" data-index="${i}" tabindex="0" role="button">${movieCardHtml(item)}</div>`).join('');
  const loadMore = state.hasMore ? '<button type="button" class="btn btn-secondary load-more-btn">Load more</button>' : '';
  el.innerHTML = cards + loadMore;
  el.querySelectorAll('.poster-card').forEach((card) => {
    card.addEventListener('click', () => state.onSelect(state.items[Number(card.dataset.index)]));
  });
  const btn = el.querySelector('.load-more-btn');
  if (btn) btn.addEventListener('click', () => loadMoreResults(resultsId));
}

async function fetchAndRenderPage(resultsId, page) {
  const state = gridState.get(resultsId);
  const result = await state.fetchPage(page);
  state.items = page === 1 ? result.results : state.items.concat(result.results);
  state.page = page;
  state.hasMore = result.has_more;
  renderMediaGrid(resultsId);
}

async function loadMoreResults(resultsId) {
  const state = gridState.get(resultsId);
  const btn = document.getElementById(resultsId).querySelector('.load-more-btn');
  if (btn) {
    btn.disabled = true;
    btn.textContent = 'Loading...';
  }
  try {
    await fetchAndRenderPage(resultsId, state.page + 1);
  } catch (e) {
    if (btn) {
      btn.disabled = false;
      btn.textContent = 'Load more';
    }
    console.error('load more error', e);
  }
}

// Spotlight/Trending/genre-rows are "browse everything" content — once a
// search or genre/year filter actually has something to show, keeping all
// of that visible alongside the filtered grid is what made "See all X"
// feel cluttered rather than a clean, focused result. Hidden while a filter
// is active, restored once it's cleared back to "All genres"/empty search.
function setBrowseExtrasVisible(resultsId, visible) {
  const prefix = resultsId.replace('-search-results', '');
  ['spotlight-card', 'trending-section', 'genre-rows'].forEach((suffix) => {
    document.getElementById(`${prefix}-${suffix}`)?.classList.toggle('hidden', !visible);
  });
}

async function searchMedia(mediaType, inputId, resultsId, onSelect) {
  const query = document.getElementById(inputId).value;
  const el = document.getElementById(resultsId);
  if (!query.trim()) {
    gridState.delete(resultsId);
    el.innerHTML = '';
    setBrowseExtrasVisible(resultsId, true);
    return;
  }
  setBrowseExtrasVisible(resultsId, false);
  gridState.set(resultsId, {
    items: [],
    page: 0,
    hasMore: false,
    onSelect,
    fetchPage: (page) =>
      api.searchMovies(query, getTmdbKey(), page).then((r) => ({ results: r.results.filter((m) => m.media_type === mediaType), has_more: r.has_more })),
  });
  try {
    el.innerHTML = skeletonGrid(10);
    await fetchAndRenderPage(resultsId, 1);
  } catch (e) {
    renderError(el, `Error: ${e}`, () => searchMedia(mediaType, inputId, resultsId, onSelect));
  }
}

function populateYearSelect(selectId) {
  const select = document.getElementById(selectId);
  const currentYear = new Date().getFullYear();
  select.innerHTML =
    '<option value="">All years</option>' +
    Array.from({ length: currentYear - 1949 }, (_, i) => currentYear - i)
      .map((y) => `<option value="${y}">${y}</option>`)
      .join('');
}

async function loadDiscoverResults(mediaType, genreSelectId, yearSelectId, resultsId, onSelect) {
  const genreId = document.getElementById(genreSelectId).value || undefined;
  const year = document.getElementById(yearSelectId).value || undefined;
  const el = document.getElementById(resultsId);
  if (!genreId && !year) {
    gridState.delete(resultsId);
    el.innerHTML = '';
    setBrowseExtrasVisible(resultsId, true);
    return;
  }
  setBrowseExtrasVisible(resultsId, false);
  gridState.set(resultsId, { items: [], page: 0, hasMore: false, onSelect, fetchPage: (page) => api.discoverMedia(mediaType, genreId, year, getTmdbKey(), page) });
  try {
    el.innerHTML = skeletonGrid(10);
    await fetchAndRenderPage(resultsId, 1);
  } catch (e) {
    renderError(el, `Error: ${e}`, () => loadDiscoverResults(mediaType, genreSelectId, yearSelectId, resultsId, onSelect));
  }
}

// Genre options come from TMDB's own /genre list (movie and tv catalogs
// differ) rather than a hardcoded set, so they can't drift out of sync with
// what TMDB actually supports filtering by.
async function populateGenreSelect(selectId, mediaType) {
  const select = document.getElementById(selectId);
  try {
    const genres = await api.getGenres(mediaType, getTmdbKey());
    select.innerHTML = '<option value="">All genres</option>' + genres.map((g) => `<option value="${g.id}">${escapeAttr(g.name)}</option>`).join('');
  } catch (e) {
    console.error('get_genres error', e);
  }
}

function initMediaFilters(mediaType, genreSelectId, yearSelectId, searchInputId, resultsId, onSelect) {
  populateYearSelect(yearSelectId);
  populateGenreSelect(genreSelectId, mediaType);
  const reload = () => {
    document.getElementById(searchInputId).value = '';
    loadDiscoverResults(mediaType, genreSelectId, yearSelectId, resultsId, onSelect);
  };
  document.getElementById(genreSelectId).addEventListener('change', reload);
  document.getElementById(yearSelectId).addEventListener('change', reload);
}

async function loadTrendingByType(mediaType, slideshowKey, trackId) {
  slideshows[slideshowKey].loading();
  try {
    const items = (await api.getTrendingMoviesTv(getTmdbKey())).filter((m) => m.media_type === mediaType);
    slideshows[slideshowKey].setItems(items, movieCardHtml);
  } catch (e) {
    renderError(document.getElementById(trackId), `Error loading trending: ${e}`, () => loadTrendingByType(mediaType, slideshowKey, trackId));
  }
}

function seeAllCardHtml(genreName) {
  return `<div class="see-all-card"><span>See all<br>${escapeAttr(genreName)}</span><span class="see-all-arrow">&rarr;</span></div>`;
}

function genreRowCardHtml(item) {
  return item.__seeAll ? seeAllCardHtml(item.genreName) : movieCardHtml(item);
}

// Jumps to the same filtered grid the Genre dropdown itself drives — a genre
// row's "See all" card isn't a separate browse surface, just a shortcut into it.
function goToGenreResults(mediaType, genreSelectId, yearSelectId, resultsId, genreId, onSelect) {
  document.getElementById(genreSelectId).value = String(genreId);
  loadDiscoverResults(mediaType, genreSelectId, yearSelectId, resultsId, onSelect);
  document.getElementById(resultsId).scrollIntoView({ behavior: 'smooth' });
}

// A handful of well-known genres get their own "Top X" row beneath
// Trending, each ending in a "See all" card — the genres themselves still
// come from TMDB's live list (populateGenreSelect), this just picks which
// few are worth a dedicated row instead of showing all ~19.
const FEATURED_GENRE_NAMES = ['Action', 'Comedy', 'Horror', 'Animation', 'Drama'];

// TMDB's tv genre list has no "Action" or "Horror" entry — tv's closest is
// "Action & Adventure", and it has nothing for Horror at all — so an exact
// name match against FEATURED_GENRE_NAMES silently drops those rows for tv.
function matchesFeaturedGenre(name, g) {
  return g.name === name || (name === 'Action' && g.name === 'Action & Adventure');
}

async function loadGenreRows(mediaType, rowsContainerId, genreSelectId, yearSelectId, resultsId, onSelect) {
  const container = document.getElementById(rowsContainerId);
  let genres;
  try {
    genres = await api.getGenres(mediaType, getTmdbKey());
  } catch (e) {
    return; // no TMDB key yet, or a transient error — Trending above already covers that message
  }
  const featured = FEATURED_GENRE_NAMES.map((name) => genres.find((g) => matchesFeaturedGenre(name, g))).filter(Boolean);

  container.innerHTML = featured
    .map(
      (g) => `
      <div class="section-row">
        <h3 class="sidebar-heading">Top ${escapeAttr(g.name)}</h3>
        <div class="slide-nav-group">
          <button class="slide-nav" data-target="genre-${mediaType}-${g.id}" data-dir="-1" aria-label="Previous">&#8249;</button>
          <button class="slide-nav" data-target="genre-${mediaType}-${g.id}" data-dir="1" aria-label="Next">&#8250;</button>
        </div>
      </div>
      <div class="slide-viewport"><div id="genre-track-${mediaType}-${g.id}" class="slide-track"></div></div>
    `
    )
    .join('');

  // Each row's data is independent, so fetch all of them in parallel instead
  // of stacking N sequential round trips.
  await Promise.all(
    featured.map(async (g) => {
      // Movie and tv genre ids overlap (Comedy is 35 in both) — mediaType has
      // to be part of the key, or the second tab visited this session steals
      // the first tab's slideshow instance and DOM node (genre-track-35 isn't
      // unique across tabs either, same fix applies to it above).
      const key = `genre-${mediaType}-${g.id}`;
      slideshows[key] = createSlideshow(document.getElementById(`genre-track-${mediaType}-${g.id}`), {
        onSelect: (item) => (item.__seeAll ? goToGenreResults(mediaType, genreSelectId, yearSelectId, resultsId, g.id, onSelect) : onSelect(item)),
      });
      slideshows[key].loading();
      try {
        const page = await api.discoverMedia(mediaType, g.id, undefined, getTmdbKey(), 1);
        slideshows[key].setItems([...page.results, { __seeAll: true, genreName: g.name }], genreRowCardHtml);
      } catch (e) {
        renderError(document.getElementById(`genre-track-${mediaType}-${g.id}`), `Error loading ${g.name}: ${e}`, () => {});
      }
    })
  );
}

function loadMoviesTab() {
  if (moviesLoaded) return;
  moviesLoaded = true;
  loadTrendingByType('movie', 'trendingMovies', 'trending-movies-track');
  loadMediaSpotlight('movie', 'movie-spotlight-card', selectMovie);
  loadGenreRows('movie', 'movie-genre-rows', 'movie-genre-filter', 'movie-year-filter', 'movie-search-results', selectMovie);
}

function loadSeriesTab() {
  if (seriesLoaded) return;
  seriesLoaded = true;
  loadTrendingByType('tv', 'trendingSeries', 'trending-series-track');
  loadMediaSpotlight('tv', 'series-spotlight-card', selectSeries);
  loadGenreRows('tv', 'series-genre-rows', 'series-genre-filter', 'series-year-filter', 'series-search-results', selectSeries);
}

function mediaDetailsHtml(item, subParts, overview) {
  const genres = (item.genres || []).map((g) => `<span class="tag tag-outline">${escapeAttr(g)}</span>`).join('');
  return `
    <div class="media-details">
      ${item.image_url ? `<img src="${item.image_url}" alt="${escapeAttr(item.title)}" />` : ''}
      <div class="media-details-info">
        <h3>${item.title}</h3>
        <p class="poster-sub">${subParts.join(' · ')}</p>
        ${countdownLineHtml(item)}
        ${genres ? `<div class="genre-chips">${genres}</div>` : ''}
        <p class="media-details-overview">${overview || ''}</p>
      </div>
    </div>
  `;
}

function movieSubParts(item) {
  const sub = [];
  if (item.year) sub.push(item.year);
  if (item.rating) sub.push(`★ ${item.rating.toFixed(1)}`);
  return sub;
}

function selectMovie(movie) {
  showView('movie-details');
  setDetailsBackdrop('movie-details-backdrop', movie.image_url);
  document.getElementById('movie-details-container').innerHTML = mediaDetailsHtml(movie, movieSubParts(movie), movie.overview);
  document.getElementById('movie-watch-btn').onclick = () => selectMovieToWatch(movie);
  wireWishlistButton('movie-wishlist-btn', {
    id: `movie:${movie.title}`,
    kind: 'movie',
    title: movie.title,
    year: movie.year,
    image_url: movie.image_url,
    tmdb_id: movie.tmdb_id,
    release_date: movie.release_date,
  });
  wireWatchedButton('movie-watched-btn', { id: `movie:${movie.title}`, kind: 'movie', title: movie.title });
  loadMediaRecommendations('movie', movie.tmdb_id, 'movie-recs-track', 'movieRecs', selectMovie);
}

async function selectSeries(series) {
  showView('series-details');
  setDetailsBackdrop('series-details-backdrop', series.image_url);
  document.getElementById('series-details-container').innerHTML = mediaDetailsHtml(series, movieSubParts(series), series.overview);
  wireWishlistButton('series-wishlist-btn', {
    id: `series:${series.title}`,
    kind: 'series',
    title: series.title,
    year: series.year,
    image_url: series.image_url,
    tmdb_id: series.tmdb_id,
  });
  wireWatchedButton('series-watched-btn', { id: `series:${series.title}`, kind: 'series', title: series.title });
  loadMediaRecommendations('tv', series.tmdb_id, 'series-recs-track', 'seriesRecs', selectSeries);

  const seasonsEl = document.getElementById('series-season-buttons');
  const episodesEl = document.getElementById('series-episodes-list');
  seasonsEl.innerHTML = '<p class="muted-center">Loading seasons...</p>';
  episodesEl.innerHTML = '';

  try {
    const seasons = await api.getTvSeasons(series.tmdb_id, getTmdbKey());
    if (!seasons.length) {
      seasonsEl.innerHTML = '<p class="muted-center">No season data found</p>';
      return;
    }
    seasonsEl.innerHTML = seasons
      .map((s, i) => `<button class="season-btn${i === 0 ? ' active' : ''}" data-season="${s.season_number}">${s.name || `Season ${s.season_number}`}</button>`)
      .join('');
    seasonsEl.querySelectorAll('.season-btn').forEach((btn) => {
      btn.addEventListener('click', () => {
        seasonsEl.querySelectorAll('.season-btn').forEach((b) => b.classList.remove('active'));
        btn.classList.add('active');
        loadSeriesEpisodes(series, Number(btn.dataset.season));
      });
    });
    loadSeriesEpisodes(series, seasons[0].season_number);
  } catch (e) {
    renderError(seasonsEl, `Error loading seasons: ${e}`, () => selectSeries(series));
  }
}

async function loadSeriesEpisodes(series, seasonNumber) {
  const episodesEl = document.getElementById('series-episodes-list');
  episodesEl.innerHTML = '<p class="muted-center">Loading episodes...</p>';
  try {
    const episodes = await api.getSeasonEpisodes(series.tmdb_id, seasonNumber, getTmdbKey());
    const numbers = episodes.map((ep) => ep.episode_number);
    episodesEl.innerHTML = episodes
      .map((ep) => `<button class="episode-btn" data-episode="${ep.episode_number}" title="${escapeAttr(ep.name)}">${ep.episode_number}</button>`)
      .join('') || '<p class="muted-center">No episodes found</p>';

    episodesEl.querySelectorAll('.episode-btn').forEach((btn) => {
      btn.addEventListener('click', function () {
        episodesEl.querySelectorAll('.episode-btn').forEach((b) => b.classList.remove('active'));
        this.classList.add('active');
        selectSeriesEpisodeToWatch(series, seasonNumber, Number(this.dataset.episode), numbers);
      });
    });
  } catch (e) {
    renderError(episodesEl, `Error loading episodes: ${e}`, () => loadSeriesEpisodes(series, seasonNumber));
  }
}

// ---------- Watching ----------

const QUALITY_LABELS = { best: 'Best quality', '1080': '1080p', '720': '720p', '480': '480p', '360': '360p', worst: 'Worst quality' };

// Single place the player-panel status line is written from, so the accent
// dot next to it (see .watch-status-line in styles.css) always matches the
// text's error/normal state instead of only the text itself turning red.
function setPlayerStatus(text, { error = false, html = false } = {}) {
  const el = document.getElementById('player-status');
  if (html) el.innerHTML = text;
  else el.textContent = text;
  document.getElementById('watch-status-line').classList.toggle('is-error', error);
}

function getSelectedQuality() {
  return document.querySelector('input[name="quality"]:checked')?.value || 'best';
}

function setSelectedQuality(quality) {
  const el = document.querySelector(`input[name="quality"][value="${quality}"]`);
  if (el) el.checked = true;
}

function getLastQuality() {
  return localStorage.getItem('trela_last_quality') || '';
}

function setQualitySelectorVisible(visible) {
  document.getElementById('quality-select-wrap').style.display = visible ? '' : 'none';
}

// Movies/series don't get a generic best/1080/720/... picker — MovieBox's
// available resources vary per title (sometimes several encodes at the same
// resolution), so the Watching page fetches and lists the real options.
let currentStreamOptions = [];
// Bumped on every loadStreamOptions call so a slow/stale request (user backed
// out or hit Next Episode while it was in flight) can detect it's no longer
// the latest and bail instead of overwriting currentStreamOptions with the
// wrong title's sources.
let streamOptionsRequestId = 0;

function setStreamOptionsVisible(visible) {
  document.getElementById('stream-options-wrap').classList.toggle('hidden', !visible);
}

function getSelectedStreamOption() {
  const idx = document.getElementById('stream-options-seg').value;
  return currentStreamOptions[idx !== '' ? Number(idx) : 0];
}

// Download-and-play is MovieBox-only — used to be decided once from the
// global source toggle, but with both sources merged into one list it has to
// track whichever option is currently selected instead (see
// initStreamOptionsSeg's change listener).
function updateDownloadEligibility() {
  const option = getSelectedStreamOption();
  document.getElementById('download-btn').disabled = !option || !!option.fourk_release;
}

// MovieBox and 4KHDHub options are merged into one flat list (see
// loadStreamOptions) — each StreamOption already self-tags which source it
// came from via fourk_release, so grouping/labeling just reads that instead
// of trusting a single global "active source" the way it used to.
function sourceLabel(option) {
  return option.fourk_release ? '4KHDHub' : 'MovieBox';
}

// mirrors is 0 for MovieBox (no such concept there — see StreamOption in
// models.rs), so this only ever renders for 4KHDHub options, matching
// moviebox-tui's own "N mirrors" per-encode display.
function mirrorLabel(option) {
  return option.mirrors ? ` · ${option.mirrors} mirror${option.mirrors === 1 ? '' : 's'}` : '';
}

// 4KHDHub's uploader is that release's full filename (see extract_stream_options
// in movplayer.rs), e.g. "Supernatural S04E06 1080p BluRay REMUX AVC
// (EPSiLON-4kHdHub.com).mkv" — title/resolution are already implied by
// context and codec's already shown separately, so the only part worth
// surfacing inline is the release-group tag in the trailing parens (the
// actual reason fileName was preferred over uploadBy in the first place: it's
// what distinguishes two same-resolution/codec releases). MovieBox's uploader
// has no parens and is returned as-is; the full string is still in the row's
// title tooltip either way.
function shortUploader(option) {
  const match = option.uploader.match(/\(([^)]+)\)[^()]*$/);
  return match ? match[1].replace(/[-.]?4khdhub\.com$/i, '') : option.uploader;
}

// A native <select>, same as the Language/Subtitle rows right next to it —
// <optgroup> gives the resolution grouping for free (no custom popover/caret
// to maintain, no manual width-vs-overflow fight: the OS renders the open
// list, so a long label just fits the way every other select's does).
// Options come back sorted by resolution (desc), so a group is opened each
// time it changes rather than needing a separate grouping pass.
function streamOptionsHtml(options) {
  let lastResolution = null;
  let html = '';
  options.forEach((o, i) => {
    if (o.resolution !== lastResolution) {
      if (lastResolution !== null) html += '</optgroup>';
      html += `<optgroup label="${o.resolution}p">`;
      lastResolution = o.resolution;
    }
    html += `<option value="${i}">${escapeAttr(sourceLabel(o))} · ${escapeAttr(o.codec)} · ${escapeAttr(o.size)}${mirrorLabel(o)} · ${escapeAttr(shortUploader(o))}</option>`;
  });
  if (lastResolution !== null) html += '</optgroup>';
  return html;
}

// Wired once via delegation on #stream-options-seg (see initStreamOptionsSeg)
// since this innerHTML gets replaced on every episode/movie change.
async function loadStreamOptions(title, season, episode, subjectId, year) {
  const requestId = ++streamOptionsRequestId;
  const seg = document.getElementById('stream-options-seg');
  const playBtn = document.getElementById('play-btn');
  const downloadBtn = document.getElementById('download-btn');
  currentStreamOptions = [];
  seg.innerHTML = '';
  playBtn.disabled = true;
  downloadBtn.disabled = true;
  setPlayerStatus('Loading sources...');

  // Both sources are fetched together and merged (Promise.allSettled, not
  // Promise.all) so one being down — e.g. MovieBox flaky — doesn't hide
  // options the other source still has.
  //
  // subjectId === null means prepareMovieboxSubject's own MovieBox search
  // already just failed for this exact title — calling getStreamOptions
  // anyway would only re-run that identical search a second time (it does
  // its own subject lookup whenever subject_id is missing), paying a real
  // multi-second failure cost twice for no chance of a different outcome.
  const [moviebox, fourk] = await Promise.allSettled([
    subjectId === null ? Promise.reject(new Error('MovieBox search failed — see above.')) : api.getStreamOptions(title, season, episode, subjectId, year),
    api.getFourkStreamOptions(title, season, episode, year),
  ]);
  if (requestId !== streamOptionsRequestId) return; // superseded by a newer call while these were in flight

  // Each source's own list is already resolution-sorted, but concatenating
  // them isn't — without re-sorting, the first (pre-selected) group is
  // whichever source happens to come first in the array, not the actual
  // best resolution across both (e.g. MovieBox topping out at 720p would
  // still win the default over a 1080p 4KHDHub option).
  currentStreamOptions = [
    ...(moviebox.status === 'fulfilled' ? moviebox.value : []),
    ...(fourk.status === 'fulfilled' ? fourk.value : []),
  ].sort((a, b) => b.resolution - a.resolution);

  if (!currentStreamOptions.length) {
    const firstError = moviebox.status === 'rejected' ? moviebox.reason : fourk.reason;
    setPlayerStatus(`Error: ${firstError}`, { error: true });
    return;
  }

  seg.innerHTML = streamOptionsHtml(currentStreamOptions);
  playBtn.disabled = false;
  updateDownloadEligibility();
  const selected = currentStreamOptions[0];
  setPlayerStatus(`${currentStreamOptions.length} source${currentStreamOptions.length === 1 ? '' : 's'} found · ${sourceLabel(selected)} ${selected.resolution}p selected`);
  refreshSubtitleOptions();
  updateWatchMetaRow();
  updateSettingsSummary();
}

// Bound once — loadStreamOptions only ever replaces the select's innerHTML
// (its options), never the select element itself, so this keeps working
// across every episode/movie change with no per-render rewiring needed.
function initStreamOptionsSeg() {
  const seg = document.getElementById('stream-options-seg');
  seg.addEventListener('change', () => {
    updateDownloadEligibility();
    refreshSubtitleOptions();
    updateWatchMetaRow();
    updateSettingsSummary();
  });
}

// Quality/language/source/subtitle live inline under the player now, collapsed
// by default — open/closed state persists for the session (not per title), and
// the header always shows a summary of the current selection so collapsing is
// safe (see styles.css .watch-settings-*).
function setSettingsOpen(open, persist = true) {
  document.getElementById('watch-settings-body').classList.toggle('hidden', !open);
  document.getElementById('watch-settings-header').setAttribute('aria-expanded', open ? 'true' : 'false');
  document.getElementById('watch-settings-chevron').textContent = open ? 'Hide ⌃' : 'Change ⌄';
  if (persist) sessionStorage.setItem('trela_watch_settings_open', open ? '1' : '0');
}

function applySettingsOpenState() {
  setSettingsOpen(sessionStorage.getItem('trela_watch_settings_open') === '1', false);
}

function initWatchSettingsToggle() {
  const header = document.getElementById('watch-settings-header');
  const toggle = () => setSettingsOpen(document.getElementById('watch-settings-body').classList.contains('hidden'));
  header.addEventListener('click', toggle);
  header.addEventListener('keydown', (e) => {
    if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault();
      toggle();
    }
  });
}

// The player panel's meta row states what the play button will actually
// launch with — recomputed on entry and whenever a setting changes, never
// left as static copy (see README "Watch Episode page redesign").
function updateWatchMetaRow() {
  if (!pendingPlay) return;
  if (pendingPlay.kind === 'anime') {
    renderMetaParts('watch-player-meta', [QUALITY_LABELS[getSelectedQuality()] || 'Best quality', 'ani-cli']);
    return;
  }
  const option = getSelectedStreamOption();
  if (!option) {
    document.getElementById('watch-player-meta').innerHTML = '';
    return;
  }
  renderMetaParts('watch-player-meta', [
    `${option.resolution}p`,
    sourceLabel(option),
    option.codec,
    option.size,
    option.mirrors ? `${option.mirrors} mirror${option.mirrors === 1 ? '' : 's'}` : '',
  ]);
}

// The settings header's summary is what makes collapsing the panel safe —
// it must always reflect live state, so it's recomputed alongside the meta
// row above.
function updateSettingsSummary() {
  const el = document.getElementById('watch-settings-summary');
  if (!pendingPlay) return;
  if (pendingPlay.kind === 'anime') {
    el.textContent = QUALITY_LABELS[getSelectedQuality()] || 'Best quality';
    return;
  }
  const option = getSelectedStreamOption();
  const parts = [];
  if (option) parts.push(`${option.resolution}p`, sourceLabel(option));
  const langText = document.getElementById('watch-language-select').selectedOptions[0]?.textContent;
  if (langText && langText !== 'Default') parts.push(langText);
  const subText = document.getElementById('watch-subtitle-select').selectedOptions[0]?.textContent;
  parts.push(subText && subText !== 'None' ? subText : 'No subtitles');
  el.textContent = parts.join(' · ');
}

// ---------- Movies/series: language picker + subtitle-select download fallback ----------

// Caches the MovieBox subject (and its dub/language alternates) per title so
// stepping between episodes of the same series doesn't re-resolve it, but a
// different title always does.
let currentWatchSubject = null; // { title, subject_id, languages }
let selectedLanguageSubjectId = null; // user's pick for the current title, or null = default

function populateLanguageSelect(languages, selectedId) {
  const wrap = document.getElementById('watch-language-wrap');
  const select = document.getElementById('watch-language-select');
  if (!languages.length) {
    wrap.classList.add('hidden');
    select.innerHTML = '';
    return;
  }
  wrap.classList.remove('hidden');
  select.innerHTML =
    '<option value="">Default</option>' + languages.map((l) => `<option value="${escapeAttr(l.subject_id)}">${escapeAttr(l.name)}</option>`).join('');
  select.value = selectedId || '';
}

// Resolves (or reuses) the MovieBox subject for `title` — only to list its
// dub/language alternates, not to pin which subject get_stream_options uses.
// A title can be indexed multiple times on MovieBox (e.g. an exact-title
// listing that only has Season 1 alongside a separate "S1-S3" listing that
// actually has season 3), so unless the user explicitly picked a language,
// this returns undefined and lets get_stream_options run its own
// season/episode-aware search instead of pinning the (possibly wrong) exact-
// title match get_movie_subject found.
async function prepareMovieboxSubject(title, year) {
  if (!currentWatchSubject || currentWatchSubject.title !== title) {
    const info = await api.getMovieSubject(title, year);
    currentWatchSubject = { title, subject_id: info.subject_id, languages: info.languages };
    selectedLanguageSubjectId = null;
  }
  populateLanguageSelect(currentWatchSubject.languages, selectedLanguageSubjectId);
  return selectedLanguageSubjectId || undefined;
}

function initWatchLanguageSelect() {
  document.getElementById('watch-language-select').addEventListener('change', (e) => {
    if (!pendingPlay || !currentWatchSubject) return;
    selectedLanguageSubjectId = e.target.value || null;
    const season = pendingPlay.kind === 'series' ? pendingPlay.season : 0;
    const episode = pendingPlay.kind === 'series' ? pendingPlay.episode : 0;
    loadStreamOptions(pendingPlay.title, season, episode, selectedLanguageSubjectId || undefined, pendingPlay.year);
  });
}

function getLastSubtitleLang() {
  return localStorage.getItem('trela_last_subtitle_lang') || '';
}

// Subtitles are keyed by resource (subject_id + resource_id), so they're
// re-fetched whenever the selected stream option changes, not just once.
async function refreshSubtitleOptions() {
  const select = document.getElementById('watch-subtitle-select');
  select.innerHTML = '<option value="">None</option>';
  const option = getSelectedStreamOption();
  // 4KHDHub subtitles (if any) are attached automatically when the link is
  // resolved at play time — there's no per-resource picker for that source.
  if (!option || option.fourk_release) return;
  try {
    const subtitles = await api.getSubtitleOptions(option.subject_id, option.resource_id);
    select.innerHTML += subtitles.map((s) => `<option value="${escapeAttr(s.url)}">${escapeAttr(s.language)}</option>`).join('');
    // Subtitles have no stable code across titles, just a human-readable
    // language name — match the last one picked by that name.
    const preferred = getLastSubtitleLang();
    const match = preferred && Array.from(select.options).find((o) => o.textContent === preferred);
    if (match) select.value = match.value;
  } catch (e) {
    console.error('get_subtitle_options error', e);
  }
}

function initPlaybackMemory() {
  document.querySelectorAll('input[name="quality"]').forEach((el) => {
    el.addEventListener('change', (e) => {
      localStorage.setItem('trela_last_quality', e.target.value);
      updateWatchMetaRow();
      updateSettingsSummary();
    });
  });
  document.getElementById('watch-subtitle-select').addEventListener('change', (e) => {
    localStorage.setItem('trela_last_subtitle_lang', e.target.selectedOptions[0]?.textContent || '');
    updateSettingsSummary();
  });
}

function setDownloadUiVisible(visible) {
  document.getElementById('watch-subtitle-wrap').classList.toggle('hidden', !visible);
  document.getElementById('download-wrap').classList.toggle('hidden', !visible);
}

function setDownloadProgress(percent, statusText) {
  document.getElementById('download-progress-wrap').classList.toggle('hidden', percent === null);
  if (percent !== null) document.getElementById('download-progress-fill').style.width = `${Math.min(100, Math.max(0, percent))}%`;
  document.getElementById('download-progress-status').textContent = statusText || '';
}

function formatBytes(bytes) {
  if (!bytes) return '0 MB';
  const mb = bytes / 1024 / 1024;
  return mb > 1024 ? `${(mb / 1024).toFixed(1)} GB` : `${mb.toFixed(0)} MB`;
}

// Some MovieBox links stream poorly (no Range support, throttled, etc.) —
// downloading the whole file first and playing the local copy is the
// resilient fallback. The download itself (and the auto-play once it
// finishes) happens server-side; see initDownloadEvents for progress.
async function startDownloadAndPlay() {
  if (!pendingPlay || (pendingPlay.kind !== 'movie' && pendingPlay.kind !== 'series')) return;
  const option = getSelectedStreamOption();
  if (!option || option.fourk_release) return; // download fallback is MovieBox-only for now

  const downloadBtn = document.getElementById('download-btn');
  const subtitleUrl = document.getElementById('watch-subtitle-select').value || undefined;
  const windowTitle = pendingPlay.kind === 'movie' ? pendingPlay.title : `${pendingPlay.title} S${pendingPlay.season}E${pendingPlay.episode}`;
  const season = pendingPlay.kind === 'series' ? pendingPlay.season : 0;
  const episode = pendingPlay.kind === 'series' ? pendingPlay.episode : 0;

  downloadBtn.disabled = true;
  setDownloadProgress(0, 'Starting download...');
  try {
    await api.startDownload(option.resource_link, pendingPlay.title, season, episode, windowTitle, subtitleUrl);
    // Mirrors the saveContinueWatching call in startPlayback's movie/series
    // branch — start_download spawns mpv (with the same watch_id scheme)
    // once the download finishes, so this path needs to join Continue
    // Watching too or its progress-bar events never have an entry to update.
    const watchId = pendingPlay.kind === 'movie' ? `movie:${pendingPlay.title}` : `series:${pendingPlay.title}`;
    saveContinueWatching(
      pendingPlay.kind === 'movie'
        ? { id: watchId, kind: 'movie', title: pendingPlay.title, year: pendingPlay.year, image_url: pendingPlay.image_url }
        : {
            id: watchId,
            kind: 'series',
            title: pendingPlay.title,
            year: pendingPlay.year,
            season: pendingPlay.season,
            episode: pendingPlay.episode,
            tmdb_id: pendingPlay.tmdb_id,
            image_url: pendingPlay.image_url,
          }
    );
  } catch (e) {
    downloadBtn.disabled = false;
    setDownloadProgress(null, '');
    setPlayerStatus(`Error: ${e}`, { error: true });
  }
}

function initDownloadEvents() {
  onEvent('download-progress', (payload) => {
    const percent = payload.total ? (payload.downloaded / payload.total) * 100 : 0;
    const total = payload.total ? formatBytes(payload.total) : 'unknown size';
    setDownloadProgress(percent, `${formatBytes(payload.downloaded)} / ${total} · ${formatBytes(payload.bytes_per_second)}/s`);
  });
  onEvent('download-complete', (payload) => {
    setDownloadProgress(100, 'Download complete — playing...');
    document.getElementById('download-btn').disabled = false;
    setPlayerStatus(`Downloaded to ${payload.path}`);
  });
  onEvent('download-error', (payload) => {
    setDownloadProgress(null, '');
    document.getElementById('download-btn').disabled = false;
    setPlayerStatus(`Download error: ${payload.message}`, { error: true });
  });
}

// mpv reports position/duration once (see mpv_progress.rs's track()) when
// it closes — keyed by watch_id, the same id saveContinueWatching already
// used for this entry, not by "whatever pendingPlay currently is" (mpv can
// still be open, and later closed, well after the user has navigated
// elsewhere in the app).
function initPlaybackProgressEvents() {
  onEvent('playback-progress', (payload) => {
    // The duration IPC query can occasionally get an implausible answer back
    // from mpv — same "just don't show it if it looks wrong" philosophy as
    // the null checks below.
    if (payload.position_seconds == null || payload.duration_seconds == null) return;
    if (payload.position_seconds > payload.duration_seconds) return;
    updateContinueWatchingProgress(payload.watch_id, payload.position_seconds, payload.duration_seconds);
    renderContinueWatching();
  });
}

function setWatchingBackTarget(target, crumbTitle) {
  const btn = document.getElementById('watching-back-btn');
  btn.dataset.back = target;
  btn.innerHTML = crumbTitle ? `&larr; ${crumbTitle}` : '&larr; Back';
}

// Navigates to the Watching page for the chosen episode but does not start
// playback yet — the user picks a quality and clicks Play, so the
// resolution choice stays a real GUI control instead of a terminal menu.
function setEpisodeNav(nav) {
  const wrap = document.getElementById('episode-nav');
  const prevBtn = document.getElementById('episode-prev-btn');
  const nextBtn = document.getElementById('episode-next-btn');
  wrap.classList.toggle('hidden', !nav);
  if (!nav) return;
  prevBtn.disabled = !nav.onPrev;
  nextBtn.disabled = !nav.onNext;
  prevBtn.onclick = nav.onPrev;
  nextBtn.onclick = nav.onNext;
}

// Builds Prev/Next handlers from the full episode-number list a page already
// fetched, so the Watching page can step between episodes without a round
// trip back to Details.
function buildEpisodeNav(numbers, current, onGo) {
  const i = numbers.indexOf(current);
  if (i === -1) return null;
  return {
    onPrev: i > 0 ? () => onGo(numbers[i - 1]) : null,
    onNext: i < numbers.length - 1 ? () => onGo(numbers[i + 1]) : null,
  };
}

// Fallback for when the full episode-number list isn't known (Continue
// Watching / hero resume skip Details, so they never fetch one) — just
// steps +/-1. An invalid episode already surfaces a clear error from
// watchEpisode/get_stream_options, so Next has no need to know the real
// upper bound. Preserves `current`'s type (anime episode numbers are
// strings, series episode numbers are plain numbers).
function sequentialNav(current, onGo) {
  const n = Number(current);
  if (!Number.isFinite(n)) return null;
  const cast = (v) => (typeof current === 'string' ? String(v) : v);
  return { onPrev: n > 1 ? () => onGo(cast(n - 1)) : null, onNext: () => onGo(cast(n + 1)) };
}

function enterWatching(play, { showQuality = false, backTarget, crumbTitle, kicker, title, quality, nav }) {
  pendingPlay = play;
  setQualitySelectorVisible(showQuality);
  setStreamOptionsVisible(!showQuality);
  setDownloadUiVisible(!showQuality);
  document.getElementById('watch-language-wrap').classList.add('hidden');
  // Movie/series playability isn't known yet — resolving the MovieBox
  // subject/stream options is async, so Play/Download start disabled and
  // loadStreamOptions re-enables them once real data has loaded for this
  // title (otherwise a click during that gap could act on the previous
  // title's stale stream options).
  document.getElementById('download-btn').disabled = true;
  setDownloadProgress(null, '');
  setWatchingBackTarget(backTarget, crumbTitle);
  showView('watching');

  document.getElementById('watch-kicker-row').classList.toggle('hidden', !kicker);
  document.getElementById('watch-kicker-text').textContent = kicker || '';
  document.getElementById('watch-title').textContent = title;
  if (showQuality) setSelectedQuality(quality || getLastQuality() || 'best');

  document.getElementById('play-btn').disabled = !showQuality;
  setPlayerStatus(showQuality ? 'Ready to play — opens in mpv' : 'Loading sources...');
  setEpisodeNav(nav || null);
  applySettingsOpenState();
  updateWatchMetaRow();
  updateSettingsSummary();
}

// backOverride: { target, label } — see selectSeriesEpisodeToWatch's comment,
// same reasoning applies here for a resumed anime's Next/Prev Episode clicks.
// ---------- Watching: episode strip + sidebar (poster card, recommendations) ----------

// Renders a horizontal, scroll-snapped strip of episode cards from a list of
// { episode_number, still_url?, runtime? } — shared by anime (bare numbers,
// no per-episode art available from AniList) and series (real TMDB stills/
// runtime). `current` marks the one whose page is open right now; there's no
// per-episode watched history stored anywhere in the app, so every other
// card is left unlabeled rather than guessing at past viewing.
function renderEpisodeTrack(episodes, current, { ariaLabel, sub, onSelect }) {
  const track = document.getElementById('watch-episode-track');
  track.innerHTML = episodes
    .map((ep) => {
      const isCurrent = ep.episode_number === current;
      const style = ep.still_url ? ` style="background-image:url('${ep.still_url}')"` : '';
      return `
        <div class="watch-ep-card${isCurrent ? ' is-current' : ''}" data-episode="${ep.episode_number}" role="button" tabindex="0" aria-current="${isCurrent ? 'true' : 'false'}" aria-label="${escapeAttr(ariaLabel(ep, isCurrent))}">
          <div class="watch-ep-thumb"${style}>
            <span class="watch-ep-badge watch-ep-num">${ep.episode_number}</span>
            ${isCurrent ? '<span class="watch-ep-badge watch-ep-state">Playing</span>' : ''}
            <div class="watch-ep-progress-track"><div class="watch-ep-progress-fill" style="width:${isCurrent ? 100 : 0}%"></div></div>
          </div>
          <div class="watch-ep-meta">
            <div class="watch-ep-title">Episode ${ep.episode_number}</div>
            <div class="watch-ep-sub">${escapeAttr(sub(ep, isCurrent))}</div>
          </div>
        </div>`;
    })
    .join('');
  track.querySelectorAll('.watch-ep-card').forEach((card) => {
    card.addEventListener('click', () => onSelect(card.dataset.episode));
  });
  // scrollLeft, never scrollIntoView — the latter can drag ancestor scroll
  // containers (the page itself) along with it.
  const currentCard = track.querySelector('.watch-ep-card.is-current');
  if (currentCard) track.scrollLeft = Math.max(0, currentCard.offsetLeft - (track.clientWidth - currentCard.clientWidth) / 2);
}

// Anime has no per-episode stills/runtime from AniList — the strip only
// needs to know the total count to render 1..N, and is hidden without one
// (shallow Continue Watching/Recent Episodes entries never carry it).
function renderAnimeEpisodeStrip(anime, episodeNum, goTo) {
  const section = document.getElementById('watch-episodes');
  const total = anime.episodes;
  if (!total) {
    section.classList.add('hidden');
    return;
  }
  section.classList.remove('hidden');
  document.getElementById('watch-episodes-season-pills').innerHTML = '';
  const countEl = document.getElementById('watch-episodes-count');
  countEl.textContent = `${total} episode${total === 1 ? '' : 's'}`;
  countEl.classList.remove('hidden');
  const episodes = Array.from({ length: total }, (_, i) => ({ episode_number: String(i + 1) }));
  renderEpisodeTrack(episodes, String(episodeNum), {
    ariaLabel: (ep, isCurrent) => `Episode ${ep.episode_number}${isCurrent ? ' — Now playing' : ''}`,
    sub: (ep, isCurrent) => (isCurrent ? 'Now playing' : ''),
    onSelect: goTo,
  });
}

const seasonsCache = new Map(); // tmdb_id -> TvSeason[]
async function getCachedSeasons(tmdbId, apiKey) {
  if (!seasonsCache.has(tmdbId)) seasonsCache.set(tmdbId, await api.getTvSeasons(tmdbId, apiKey));
  return seasonsCache.get(tmdbId);
}

// jumpTo(seasonNumber, episodeNumber) performs a real navigation (same as
// clicking an episode on the Details page); season pills only change which
// season's episodes the strip is *browsing* and don't navigate by themselves.
async function renderSeriesEpisodeStrip(series, season, episode, jumpTo) {
  const section = document.getElementById('watch-episodes');
  section.classList.remove('hidden');
  document.getElementById('watch-episodes-count').classList.add('hidden');
  const pillsEl = document.getElementById('watch-episodes-season-pills');
  const track = document.getElementById('watch-episode-track');

  const showSeason = async (browsedSeason) => {
    try {
      const episodes = await api.getSeasonEpisodes(series.tmdb_id, browsedSeason, getTmdbKey());
      track.scrollLeft = 0;
      renderEpisodeTrack(episodes, browsedSeason === season ? episode : -1, {
        ariaLabel: (ep, isCurrent) =>
          `Season ${browsedSeason}, episode ${ep.episode_number}${isCurrent ? ' — Now playing' : ep.runtime ? ` — Not watched · ${ep.runtime} min` : ' — Not watched'}`,
        sub: (ep, isCurrent) => (isCurrent ? 'Now playing' : ep.runtime ? `${ep.runtime} min` : ''),
        onSelect: (num) => jumpTo(browsedSeason, Number(num)),
      });
    } catch (e) {
      section.classList.add('hidden');
    }
  };

  try {
    const seasons = await getCachedSeasons(series.tmdb_id, getTmdbKey());
    pillsEl.innerHTML = seasons
      .map(
        (s) =>
          `<span role="button" tabindex="0" class="watch-season-pill${s.season_number === season ? ' active' : ''}" data-season="${s.season_number}">${escapeAttr(s.name || 'Season ' + s.season_number)}</span>`
      )
      .join('');
    pillsEl.querySelectorAll('.watch-season-pill').forEach((pill) => {
      pill.addEventListener('click', () => {
        pillsEl.querySelectorAll('.watch-season-pill').forEach((p) => p.classList.remove('active'));
        pill.classList.add('active');
        showSeason(Number(pill.dataset.season));
      });
    });
  } catch (e) {
    pillsEl.innerHTML = '';
  }
  showSeason(season);
}

function renderWatchPosterCard({ title, imageUrl, rating, metaLine, genres }) {
  document.getElementById('watch-poster-img').style.backgroundImage = imageUrl ? `url('${imageUrl}')` : '';
  const ratingEl = document.getElementById('watch-rating-chip');
  ratingEl.textContent = rating || '';
  ratingEl.classList.toggle('hidden', !rating);
  document.getElementById('watch-side-title').textContent = title;
  document.getElementById('watch-side-meta').textContent = metaLine;
  document.getElementById('watch-side-tags').innerHTML = (genres || [])
    .slice(0, 3)
    .map((g) => `<span class="tag tag-accent">${escapeAttr(g)}</span>`)
    .join('');
}

function watchRecRowHtml(item, metaText) {
  return `
    <div class="watch-rec-row" role="button" tabindex="0" aria-label="${escapeAttr(item.title)}">
      <div class="watch-rec-thumb" style="${item.image_url ? `background-image:url('${item.image_url}')` : ''}"></div>
      <div class="watch-rec-info">
        <div class="watch-rec-title">${escapeAttr(item.title)}</div>
        <div class="watch-rec-meta">${escapeAttr(metaText)}</div>
      </div>
    </div>`;
}

// Hidden entirely below 2 results rather than padding the list out with
// nothing to show.
function renderWatchRecs(items, metaFor, onSelect) {
  const section = document.getElementById('watch-recs-section');
  const list = document.getElementById('watch-recs-list');
  if (!items || items.length < 2) {
    section.classList.add('hidden');
    list.innerHTML = '';
    return;
  }
  section.classList.remove('hidden');
  const shown = items.slice(0, 4);
  list.innerHTML = shown.map((item) => watchRecRowHtml(item, metaFor(item))).join('');
  list.querySelectorAll('.watch-rec-row').forEach((row, i) => {
    row.addEventListener('click', () => onSelect(shown[i]));
  });
}

async function loadWatchAnimeRecs(id) {
  try {
    const items = await api.getAnimeRecommendations(id);
    renderWatchRecs(
      items,
      (item) => [item.episodes ? `${item.episodes} eps` : capitalize(item.status || ''), item.score ? `★ ${item.score}` : ''].filter(Boolean).join(' · '),
      selectAnime
    );
  } catch (e) {
    renderWatchRecs([], null, null);
  }
}

async function loadWatchMediaRecs(mediaType, tmdbId, onSelect) {
  try {
    const items = await api.getRecommendations(mediaType, tmdbId, getTmdbKey());
    renderWatchRecs(items, (item) => [item.year, item.rating ? `★ ${item.rating.toFixed(1)}` : ''].filter(Boolean).join(' · '), onSelect);
  } catch (e) {
    renderWatchRecs([], null, null);
  }
}

function selectEpisode(anime, episodeNum, quality, episodeNumbers, backOverride) {
  const goTo = (num) => selectEpisode(anime, num, undefined, episodeNumbers, backOverride);
  const nav = episodeNumbers ? buildEpisodeNav(episodeNumbers, episodeNum, goTo) : sequentialNav(episodeNum, goTo);
  enterWatching(
    { kind: 'anime', anime, episodeNum },
    {
      showQuality: true,
      backTarget: backOverride?.target || 'details',
      crumbTitle: backOverride?.label || anime.title,
      kicker: anime.episodes ? `Episode ${episodeNum} of ${anime.episodes}` : `Episode ${episodeNum}`,
      title: anime.title,
      quality,
      nav,
    }
  );
  renderAnimeEpisodeStrip(anime, episodeNum, goTo);
  renderWatchPosterCard({
    title: anime.title,
    imageUrl: anime.image_url,
    rating: anime.score,
    metaLine: [anime.episodes ? `${anime.episodes} eps` : capitalize(anime.status || ''), 'Anime'].filter(Boolean).join(' · '),
    genres: anime.genres,
  });
  if (anime.id) loadWatchAnimeRecs(anime.id);
  else renderWatchRecs([], null, null);
}

async function selectMovieToWatch(movie, resumeSeconds) {
  enterWatching(
    { kind: 'movie', title: movie.title, year: movie.year, image_url: movie.image_url, resumeSeconds },
    { backTarget: 'movie-details', crumbTitle: movie.title, title: movie.title }
  );
  document.getElementById('watch-episodes').classList.add('hidden');
  renderWatchPosterCard({
    title: movie.title,
    imageUrl: movie.image_url,
    rating: movie.rating ? movie.rating.toFixed(1) : '',
    metaLine: [movie.year, 'Movie'].filter(Boolean).join(' · '),
    genres: movie.genres,
  });
  if (movie.tmdb_id) loadWatchMediaRecs('movie', movie.tmdb_id, selectMovie);
  else renderWatchRecs([], null, null);
  // Dub/language picking is MovieBox-specific machinery, but MovieBox is
  // always one of the merged sources now, so this always runs. null (not
  // undefined) specifically means "MovieBox search just failed" — see
  // loadStreamOptions, which uses that to skip a second, identical,
  // guaranteed-to-fail MovieBox attempt instead of quietly re-running it.
  const subjectId = await prepareMovieboxSubject(movie.title, movie.year).catch(() => null);
  loadStreamOptions(movie.title, 0, 0, subjectId, movie.year);
}

// backOverride: { target, label } — resumeContinueWatching passes this to
// keep Back pointed at Home across every subsequent Next/Prev Episode click
// too, not just the episode it resumed. Without threading it through goTo,
// each Next Episode click rebuilds this page via a plain selectSeriesEpisodeToWatch
// call with no override, which used to silently reset Back to Series Details.
async function selectSeriesEpisodeToWatch(series, season, episode, episodeNumbers, backOverride, resumeSeconds) {
  const goTo = (num) => selectSeriesEpisodeToWatch(series, season, num, episodeNumbers, backOverride);
  const nav = episodeNumbers ? buildEpisodeNav(episodeNumbers, episode, goTo) : sequentialNav(episode, goTo);
  enterWatching(
    { kind: 'series', title: series.title, season, episode, year: series.year, tmdb_id: series.tmdb_id, image_url: series.image_url, resumeSeconds },
    {
      backTarget: backOverride?.target || 'series-details',
      crumbTitle: backOverride?.label || series.title,
      kicker: `Season ${season} · Episode ${episode}`,
      title: series.title,
      nav,
    }
  );
  const jumpTo = (seasonNum, num) => selectSeriesEpisodeToWatch(series, seasonNum, num, undefined, backOverride);
  if (series.tmdb_id) {
    renderSeriesEpisodeStrip(series, season, episode, jumpTo);
    loadWatchMediaRecs('tv', series.tmdb_id, selectSeries);
  } else {
    document.getElementById('watch-episodes').classList.add('hidden');
    renderWatchRecs([], null, null);
  }
  renderWatchPosterCard({
    title: series.title,
    imageUrl: series.image_url,
    rating: series.rating ? series.rating.toFixed(1) : '',
    metaLine: [series.year, 'Series'].filter(Boolean).join(' · '),
    genres: series.genres,
  });
  // null vs undefined: see selectMovieToWatch's identical line.
  const subjectId = await prepareMovieboxSubject(series.title, series.year).catch(() => null);
  loadStreamOptions(series.title, season, episode, subjectId, series.year);
}

// Caches which of ani-cli's own search results is actually the right one for
// a title (and its real episode numbering) so stepping between episodes of
// the same show doesn't re-resolve every time — a different title always does.
let currentAnimeResolution = null; // { title, query, index, episodes }

// A null result (provider reachable, genuinely no matching title) and a
// thrown error (provider unreachable/down/blocked — see curl_impersonate_get
// in anidb.rs) used to both collapse into the same generic "not found"
// message here, which misreports a site outage as "this title doesn't
// exist". Only null is handled here now; a real error is left to propagate
// to startPlayback's own catch, same as every other real failure in that
// function, so the actual reason reaches the user.
async function resolveAnimeForPlayback(title, expectedEpisodes, isReleasing) {
  if (currentAnimeResolution && currentAnimeResolution.title === title) return currentAnimeResolution;
  const resolved = await api.resolveAnime(title, expectedEpisodes, isReleasing);
  if (!resolved) return null;
  currentAnimeResolution = { title, ...resolved };
  return currentAnimeResolution;
}

async function startPlayback() {
  if (!pendingPlay) return;
  // play-btn is only disabled while sources are loading (see
  // loadStreamOptions) — reuse that same flag as a re-entrancy guard so a
  // fast double-click can't fire two watch_episode/play_stream calls (two
  // mpv windows racing) while the first one is still awaiting.
  const playBtn = document.getElementById('play-btn');
  if (playBtn.disabled) return;
  playBtn.disabled = true;

  try {
    if (pendingPlay.kind === 'anime') {
      const { anime, episodeNum } = pendingPlay;
      const quality = getSelectedQuality();
      setPlayerStatus('Resolving...');
      const resolved = await resolveAnimeForPlayback(anime.title, anime.episodes, anime.status === 'Currently airing');
      if (!resolved) {
        setPlayerStatus(`Could not find "${anime.title}" — ani-cli's source doesn't have a match.`, { error: true });
        return;
      }
      // A known AniList episode count means the button clicked was local
      // (1..count) and needs mapping to the real numbering resolveAnime
      // found for the matching entry (which can be global across seasons —
      // season 6 episode 1 is really "episode 111" for some shows). Without
      // a known count (Continue Watching resume, Recent Episodes) episodeNum
      // is already whatever real number was used last time.
      let realEpisode = episodeNum;
      if (anime.episodes && resolved.episodes.length) {
        const real = resolved.episodes[Number(episodeNum) - 1];
        if (real) realEpisode = real.number;
      }
      setPlayerStatus(`Loading Episode ${episodeNum}...`);
      const result = await api.watchEpisode(resolved.query, resolved.index, realEpisode, quality);
      setPlayerStatus(`<strong>Episode ${episodeNum}</strong><br/>${result}`, { html: true });
      // Store the real (already-resolved) number so a future resume plays
      // the same episode directly without needing to re-map anything.
      saveContinueWatching({ id: anime.title, kind: 'anime', title: anime.title, episode: realEpisode, quality, status: anime.status, image_url: anime.image_url });
    } else if (pendingPlay.kind === 'movie' || pendingPlay.kind === 'series') {
      const option = getSelectedStreamOption();
      if (!option) {
        setPlayerStatus('No source selected');
        return;
      }
      const windowTitle = pendingPlay.kind === 'movie'
        ? pendingPlay.title
        : `${pendingPlay.title} S${pendingPlay.season}E${pendingPlay.episode}`;
      // Shared with saveContinueWatching below and threaded into the play calls
      // so the eventual playback-progress event (see initPlaybackProgressEvents)
      // can be matched back to this exact entry even if the user has since
      // navigated elsewhere by the time mpv actually closes.
      const watchId = pendingPlay.kind === 'movie' ? `movie:${pendingPlay.title}` : `series:${pendingPlay.title}`;
      setPlayerStatus('Starting playback...');
      // 4KHDHub's resource_link is a mirror page, not a direct video URL —
      // play_fourk_stream resolves the real link (and any headers it needs)
      // first. Its mirrors are third-party hosts that go down independently
      // per quality/codec option, so the chosen one is sent first followed by
      // the other options as fallback instead of just the one pick.
      const playViaOption = (opt) =>
        opt.fourk_release
          ? api.playFourkStream(
              [opt.fourk_release, ...currentStreamOptions.filter((o) => o !== opt && o.fourk_release).map((o) => o.fourk_release)],
              windowTitle,
              pendingPlay.resumeSeconds,
              watchId
            )
          : api.playStream(opt.resource_link, windowTitle, opt.subject_id, opt.resource_id, pendingPlay.resumeSeconds, watchId);

      try {
        setPlayerStatus(await playViaOption(option));
      } catch (primaryError) {
        // The chosen option's own mirrors are exhausted — that's usually a
        // provider-wide issue (a scraper heuristic broken by a site change,
        // not one dead mirror), so retrying more of the *same* provider
        // rarely helps. The whole point of merging MovieBox + 4KHDHub into
        // one Source list was so a dead provider doesn't have to be a dead
        // end — fall back to the other one before giving up.
        const fallbackOption = currentStreamOptions.find((o) => Boolean(o.fourk_release) !== Boolean(option.fourk_release));
        if (!fallbackOption) throw primaryError;
        setPlayerStatus(`${sourceLabel(option)} failed, trying ${sourceLabel(fallbackOption)}...`);
        setPlayerStatus(await playViaOption(fallbackOption));
      }
      // id is kind-prefixed (unlike the bare-title anime id above) so a movie
      // and an anime that happen to share a title don't overwrite each other.
      // saveContinueWatching replaces the whole entry, so without this a
      // fresh entry built here would wipe out any position/duration a prior
      // session already saved — and if mpv is later killed abruptly (or the
      // user navigates away before it exits cleanly) that wipe is never
      // overwritten with real data, permanently losing the progress bar. A
      // movie has no episode to mismatch, so its progress is always carried
      // forward; a series' watch_id has no season/episode in it, so its
      // progress is only carried forward when it's still describing the same
      // episode that's about to be saved.
      const prevEntry = getContinueWatching().find((e) => e.id === watchId);
      const keepProgress = prevEntry && (pendingPlay.kind === 'movie'
        || (prevEntry.season === pendingPlay.season && prevEntry.episode === pendingPlay.episode));
      const progress = keepProgress
        ? { positionSeconds: prevEntry.positionSeconds, durationSeconds: prevEntry.durationSeconds }
        : {};
      saveContinueWatching(
        pendingPlay.kind === 'movie'
          ? { id: watchId, kind: 'movie', title: pendingPlay.title, year: pendingPlay.year, image_url: pendingPlay.image_url, ...progress }
          : {
              id: watchId,
              kind: 'series',
              title: pendingPlay.title,
              year: pendingPlay.year,
              season: pendingPlay.season,
              episode: pendingPlay.episode,
              tmdb_id: pendingPlay.tmdb_id,
              image_url: pendingPlay.image_url,
              ...progress,
            }
      );
    }
  } catch (e) {
    setPlayerStatus(`Error: ${e}`, { error: true });
  } finally {
    playBtn.disabled = false;
  }
}

// ---------- Command palette ----------

const NAV_DESTINATIONS = [
  { view: 'home', label: 'Home' },
  { view: 'browse', label: 'Browse' },
  { view: 'movies', label: 'Movies' },
  { view: 'series', label: 'Series' },
  { view: 'about', label: 'About' },
];

let paletteAnimeResults = [];

function paletteNavHtml() {
  return NAV_DESTINATIONS.map((d) => `<div class="dialog-item" data-nav="${d.view}"><span class="dialog-item-title">${d.label}</span><span class="tag tag-outline">Go to</span></div>`).join('');
}

function wirePaletteNav(el) {
  el.querySelectorAll('[data-nav]').forEach((item) => {
    item.addEventListener('click', () => {
      closePalette();
      switchView(item.dataset.nav);
    });
  });
}

// Anime + nav only. Movies/Series have their own dedicated search grid
// (with genre/year filters and full pagination) — duplicating that into a
// 5-row palette list was a worse version of a page that already exists;
// Ctrl+K for those now just jumps to Movies/Series via "Go to" instead.
async function renderPaletteResults(query) {
  const el = document.getElementById('palette-results');
  if (!query.trim()) {
    el.innerHTML = `<div class="dialog-section-label">Go to</div>${paletteNavHtml()}`;
    wirePaletteNav(el);
    return;
  }

  paletteAnimeResults = await api.searchAnimeMeta(query).catch(() => []);

  const animeHtml = paletteAnimeResults
    .slice(0, 8)
    .map((a, i) => `<div class="dialog-item" data-anime-index="${i}"><span class="dialog-item-title">${a.title}</span><span class="tag tag-outline">Anime</span></div>`)
    .join('');

  el.innerHTML = `
    <div class="dialog-section-label">Go to</div>${paletteNavHtml()}
    ${animeHtml ? `<div class="dialog-section-label">Anime</div>${animeHtml}` : '<div class="dialog-empty">No anime matches — try Movies/Series above for movies &amp; series</div>'}
  `;
  wirePaletteNav(el);

  el.querySelectorAll('[data-anime-index]').forEach((item) => {
    item.addEventListener('click', () => {
      const anime = paletteAnimeResults[Number(item.dataset.animeIndex)];
      closePalette();
      detailsOrigin = 'home';
      selectAnime(anime);
    });
  });
}

function openPalette() {
  const input = document.getElementById('palette-input');
  document.getElementById('command-palette').classList.remove('hidden');
  input.value = '';
  renderPaletteResults('');
  input.focus();
}

function closePalette() {
  document.getElementById('command-palette').classList.add('hidden');
}

function initCommandPalette() {
  const overlay = document.getElementById('command-palette');
  const input = document.getElementById('palette-input');

  document.getElementById('header-search-trigger').addEventListener('click', openPalette);
  overlay.addEventListener('click', (e) => {
    if (e.target === overlay) closePalette();
  });
  input.addEventListener('input', debounce((e) => renderPaletteResults(e.target.value), 300));

  document.addEventListener('keydown', (e) => {
    if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'k') {
      e.preventDefault();
      overlay.classList.contains('hidden') ? openPalette() : closePalette();
    } else if (e.key === 'Escape' && !overlay.classList.contains('hidden')) {
      closePalette();
    }
  });
}

// ---------- Nav, slideshow controls, theme ----------

// Poster cards, trending rows, continue-watching items and notifications are
// plain divs (not <button>/<a>) so their click handlers are keyboard-dead by
// default. They all carry role="button" tabindex="0" in their templates —
// this one delegate makes Enter/Space activate whichever of them has focus,
// instead of adding a keydown listener at every render site.
function initKeyboardActivation() {
  document.addEventListener('keydown', (e) => {
    if (e.key !== 'Enter' && e.key !== ' ') return;
    const el = e.target.closest('div[role="button"][tabindex]');
    if (!el) return;
    e.preventDefault();
    el.click();
  });
}

function initNav() {
  document.querySelectorAll('.nav-item').forEach((btn) => {
    btn.addEventListener('click', () => switchView(btn.dataset.view));
  });
  document.querySelectorAll('.back-btn').forEach((btn) => {
    btn.addEventListener('click', () => showView(btn.dataset.back));
  });
}

function wireSearchInput(btnId, inputId, onSearch) {
  const input = document.getElementById(inputId);
  document.getElementById(btnId).addEventListener('click', onSearch);
  input.addEventListener('keypress', (e) => {
    if (e.key === 'Enter') onSearch();
  });
  input.addEventListener('input', debounce(onSearch, 400));
}

// Delegated (not bound per-button at startup) because the movie/series genre
// rows create their own .slide-nav buttons after startup, once their genre
// list has loaded.
function initSlideNav() {
  document.addEventListener('click', (e) => {
    const btn = e.target.closest('.slide-nav');
    if (!btn) return;
    const slideshow = slideshows[btn.dataset.target];
    if (slideshow) slideshow.goToPage(Number(btn.dataset.dir));
  });
}

// Shared shape behind every Settings-page "click button, run an async
// backend action, show its result as status text" control (ani-cli update,
// moviebox-tui update, clear downloads) — was hand-copied at each call site.
function wireActionButton(btnId, statusId, loadingText, action, onSuccess) {
  const btn = document.getElementById(btnId);
  const status = document.getElementById(statusId);
  btn.addEventListener('click', async () => {
    btn.disabled = true;
    status.classList.remove('error');
    status.textContent = loadingText;
    try {
      status.textContent = await action();
      if (onSuccess) onSuccess();
    } catch (e) {
      status.classList.add('error');
      status.textContent = e;
    } finally {
      btn.disabled = false;
    }
  });
}

function initAniCliUpdate() {
  // Best-effort: ani-cli isn't found on PATH until it's actually installed,
  // so a lookup failure here just leaves the version tag blank rather than
  // surfacing as an error (the update button below already reports that).
  api.getAniCliVersion().then((v) => {
    document.getElementById('ani-cli-version').textContent = `v${v}`;
  }).catch(() => {});

  wireActionButton('update-ani-cli-btn', 'update-ani-cli-status', 'Checking for update...', api.updateAniCli);
}

function initMovieboxTuiUpdate() {
  // Compiled into this binary at build time (see build.rs) — always
  // available, no network/PATH lookup involved, unlike ani-cli's version.
  api.getMovieboxTuiVersion().then((v) => {
    document.getElementById('moviebox-tui-version').textContent = `v${v}`;
  }).catch(() => {});

  wireActionButton('update-moviebox-tui-btn', 'update-moviebox-tui-status', 'Fetching latest version...', api.updateMovieboxTui);
}

function initTmdbKey() {
  const input = document.getElementById('tmdb-key-input');
  const status = document.getElementById('tmdb-key-status');
  input.value = getTmdbKey();

  document.getElementById('tmdb-key-save-btn').addEventListener('click', () => {
    setTmdbKey(input.value);
    status.textContent = input.value.trim() ? 'Saved.' : 'Cleared.';
    // loadGenreRows has no retry button (unlike Trending/Spotlight) and its
    // only caller (loadMoviesTab/loadSeriesTab) runs once per session — so a
    // key entered after visiting Movies/Series would otherwise leave those
    // genre rows permanently empty for the rest of the session. Re-running it
    // here is idempotent (rebuilds its container from scratch either way).
    if (input.value.trim()) {
      loadGenreRows('movie', 'movie-genre-rows', 'movie-genre-filter', 'movie-year-filter', 'movie-search-results', selectMovie);
      loadGenreRows('tv', 'series-genre-rows', 'series-genre-filter', 'series-year-filter', 'series-search-results', selectSeries);
    }
  });
}

function formatDownloadsInfo(info) {
  if (!info.file_count) return 'No downloaded files yet.';
  return `${info.file_count} file${info.file_count === 1 ? '' : 's'} · ${formatBytes(info.total_bytes)}`;
}

async function refreshDownloadsInfo() {
  const el = document.getElementById('downloads-info');
  try {
    el.textContent = formatDownloadsInfo(await api.getDownloadsInfo());
  } catch (e) {
    el.textContent = '';
  }
}

function initDownloadsSettings() {
  refreshDownloadsInfo();
  wireActionButton('clear-downloads-btn', 'clear-downloads-status', 'Clearing...', api.clearDownloads, refreshDownloadsInfo);
}

// Wishlist, continue-watching, and preferences — deliberately not the TMDB
// key (a personal API key, costs nothing to re-paste, and shouldn't end up
// sitting in a JSON file someone might share) or notifications (tied to
// what's airing *now*, meaningless to restore later).
const BACKUP_KEYS = ['trela_wishlist', 'trela_continue_watching', 'trela_watched', 'theme', 'trela_theme'];

function exportBackup() {
  const data = { version: 1, exportedAt: new Date().toISOString() };
  BACKUP_KEYS.forEach((key) => {
    data[key] = localStorage.getItem(key);
  });
  const blob = new Blob([JSON.stringify(data, null, 2)], { type: 'application/json' });
  const url = URL.createObjectURL(blob);
  const a = document.createElement('a');
  a.href = url;
  a.download = `trela-backup-${new Date().toISOString().slice(0, 10)}.json`;
  a.click();
  URL.revokeObjectURL(url);
}

// Reloads afterward rather than trying to live-patch every module's cached
// state (theme, wishlist/continue-watching grids) — this only runs on a
// deliberate, rare user action, so simplicity wins.
async function importBackup(file) {
  const status = document.getElementById('backup-status');
  status.classList.remove('error');
  try {
    const data = JSON.parse(await file.text());
    BACKUP_KEYS.forEach((key) => {
      if (typeof data[key] === 'string') localStorage.setItem(key, data[key]);
    });
    status.textContent = 'Backup restored — reloading...';
    setTimeout(() => window.location.reload(), 800);
  } catch (e) {
    status.classList.add('error');
    status.textContent = `Could not restore backup: ${e}`;
  }
}

function initBackup() {
  document.getElementById('export-backup-btn').addEventListener('click', exportBackup);
  const fileInput = document.getElementById('import-backup-input');
  document.getElementById('import-backup-btn').addEventListener('click', () => fileInput.click());
  fileInput.addEventListener('change', () => {
    const file = fileInput.files[0];
    if (file) importBackup(file);
    fileInput.value = '';
  });
}

const SUN_ICON_SVG =
  '<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="4"/><path d="M12 2v2M12 20v2M4.93 4.93l1.41 1.41M17.66 17.66l1.41 1.41M2 12h2M20 12h2M6.34 17.66l-1.41 1.41M19.07 4.93l-1.41 1.41"/></svg>';
const MOON_ICON_SVG =
  '<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z"/></svg>';
const CHECK_ICON_SVG =
  '<svg class="theme-option-check" width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><path d="M20 6 9 17l-5-5"/></svg>';

// ---------- Theme families ----------
// A family is a whole design system, not a palette: styles.css gives each one
// its own type, shape and shadow language. Nocturne is the original (the
// :root / :root.light blocks) and is the only family with a light/dark pair,
// so the nav's sun/moon toggle retires while a light-only family is active.
// `swatch` is the inline style the About-page picker paints its preview tile
// with — it has to be data rather than CSS because a tile previews a theme that
// isn't on. --sw-bg-image is optional; the stylesheet defaults it to none.
const THEME_FAMILIES = {
  nocturne: {
    label: 'Nocturne',
    desc: 'The default. Deep blue-violet, soft shadows.',
    lightDark: true,
    swatch: '--sw-bg:#161826;--sw-surface:#232532;--sw-line:rgba(233,233,237,0.22);--sw-chip:#9184d9;--sw-chip-2:#a7a1db',
  },
  y2k: {
    label: 'Y2K Midnight',
    desc: 'Early-2000s desktop after dark. Pixel type, hard shadows.',
    lightDark: false,
    swatch: '--sw-bg:#16122b;--sw-surface:#241c3f;--sw-line:#efe6ff;--sw-chip:#b3357f;--sw-chip-2:#ffd84d;'
      + '--sw-bg-image:linear-gradient(rgba(150,120,255,0.4) 1px, transparent 1px),'
      + 'linear-gradient(90deg, rgba(150,120,255,0.4) 1px, transparent 1px)',
  },
  terminal: {
    label: 'Terminal',
    desc: 'Phosphor console. Monospace, scanlines, glow.',
    lightDark: false,
    swatch: '--sw-bg:#0a0e0a;--sw-surface:#0f150f;--sw-line:rgba(57,255,20,0.45);--sw-chip:#39ff14;--sw-chip-2:#7fa87c;'
      + '--sw-bg-image:repeating-linear-gradient(rgba(57,255,20,0.22) 0 1px, transparent 1px 4px)',
  },
  vintage: {
    label: 'Vintage',
    desc: 'Faded film stock. Serif display, grain, polaroid frames.',
    lightDark: false,
    swatch: '--sw-bg:#f5e6c8;--sw-surface:#fffaf0;--sw-line:rgba(58,46,33,0.3);--sw-chip:#4a7b7c;--sw-chip-2:#e8b4b8;'
      + '--sw-bg-image:radial-gradient(60% 60% at 100% 0%, rgba(232,180,184,0.55), transparent 70%)',
  },
  neubrutalist: {
    label: 'Neubrutalist',
    desc: 'Black rules, primary blocks, oversized grotesque.',
    lightDark: false,
    swatch: '--sw-bg:#f6f4ec;--sw-surface:#ffffff;--sw-line:#000000;--sw-chip:#ffeb3b;--sw-chip-2:#ff5252',
  },
};
const DEFAULT_THEME_FAMILY = 'nocturne';

function currentThemeFamily() {
  return document.documentElement.dataset.theme || DEFAULT_THEME_FAMILY;
}

function applyThemeFamily(name) {
  const family = THEME_FAMILIES[name] ? name : DEFAULT_THEME_FAMILY;
  const root = document.documentElement;
  // Nocturne is the unstyled base, so it's the absence of the attribute rather
  // than a value of it — that keeps its selectors at their original specificity.
  if (family === DEFAULT_THEME_FAMILY) delete root.dataset.theme;
  else root.dataset.theme = family;
  // The light/dark class stays on <html> either way: it's what Nocturne reads
  // when you switch back, and a light-only family simply ignores it.
  // .hidden (not the hidden attribute) because .btn sets display: inline-flex,
  // which outranks the attribute's UA display: none.
  document.getElementById('theme-toggle')
    .classList.toggle('hidden', !THEME_FAMILIES[family].lightDark);
}

function applyTheme(theme) {
  const root = document.documentElement;
  root.classList.remove('dark', 'light');
  root.classList.add(theme);
  document.getElementById('theme-toggle').innerHTML = theme === 'dark' ? SUN_ICON_SVG : MOON_ICON_SVG;
}

function renderThemePicker() {
  const active = currentThemeFamily();
  document.getElementById('theme-picker').innerHTML = Object.entries(THEME_FAMILIES)
    .map(([id, family]) => {
      return `<button type="button" class="theme-option" data-theme-id="${id}" aria-pressed="${id === active}">
          <span class="theme-swatch" style="${family.swatch}">
            <span class="theme-swatch-bar"></span>
            <span class="theme-swatch-chips"><i></i><i></i></span>
          </span>
          <span class="theme-option-meta">
            <span class="theme-option-name">${family.label}${CHECK_ICON_SVG}</span>
            <span class="theme-option-desc">${family.desc}</span>
          </span>
        </button>`;
    })
    .join('');
}

function initThemePicker() {
  const picker = document.getElementById('theme-picker');
  renderThemePicker();
  picker.addEventListener('click', (e) => {
    const btn = e.target.closest('.theme-option');
    if (!btn || btn.dataset.themeId === currentThemeFamily()) return;
    applyThemeFamily(btn.dataset.themeId);
    localStorage.setItem('trela_theme', btn.dataset.themeId);
    renderThemePicker();
  });
}

function initThemeToggle() {
  const root = document.documentElement;
  const toggle = document.getElementById('theme-toggle');
  // Family first: it decides whether the light/dark toggle is even shown.
  applyThemeFamily(localStorage.getItem('trela_theme'));
  const saved = localStorage.getItem('theme');
  const initial = saved === 'dark' || saved === 'light'
    ? saved
    : (window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light');
  applyTheme(initial);

  toggle.addEventListener('click', () => {
    const isDark = root.classList.contains('dark');
    const next = isDark ? 'light' : 'dark';
    applyTheme(next);
    localStorage.setItem('theme', next);
  });
}

window.addEventListener('DOMContentLoaded', () => {
  slideshows.seasonal = createSlideshow(document.getElementById('seasonal-track'), {
    onSelect: (item) => {
      detailsOrigin = 'browse';
      selectAnime(item);
    },
  });
  slideshows.recent = createSlideshow(document.getElementById('recent-track'), {
    onSelect: (item) => {
      detailsOrigin = 'browse';
      selectAnime({ title: item.title });
    },
  });
  slideshows.trendingMovies = createSlideshow(document.getElementById('trending-movies-track'), { onSelect: selectMovie });
  slideshows.trendingSeries = createSlideshow(document.getElementById('trending-series-track'), { onSelect: selectSeries });

  wireSearchInput('movie-search-btn', 'movie-search-input', () => searchMedia('movie', 'movie-search-input', 'movie-search-results', selectMovie));
  wireSearchInput('series-search-btn', 'series-search-input', () => searchMedia('tv', 'series-search-input', 'series-search-results', selectSeries));
  initMediaFilters('movie', 'movie-genre-filter', 'movie-year-filter', 'movie-search-input', 'movie-search-results', selectMovie);
  initMediaFilters('tv', 'series-genre-filter', 'series-year-filter', 'series-search-input', 'series-search-results', selectSeries);

  document.getElementById('play-btn').addEventListener('click', startPlayback);
  document.getElementById('download-btn').addEventListener('click', startDownloadAndPlay);
  document.getElementById('nav-brand-btn').addEventListener('click', () => switchView('home'));
  document.getElementById('home-browse-all-btn').addEventListener('click', () => switchView('browse'));

  initNav();
  initKeyboardActivation();
  initSlideNav();
  initAniCliUpdate();
  initMovieboxTuiUpdate();
  initTmdbKey();
  initThemeToggle();
  initThemePicker();
  initCommandPalette();
  initBrowseLayoutToggle();
  initAnimeGenreFilter();
  initStreamOptionsSeg();
  initWatchSettingsToggle();
  initWatchLanguageSelect();
  initPlaybackMemory();
  initDownloadEvents();
  initPlaybackProgressEvents();
  initDownloadsSettings();
  initBackup();
  initNotifications();
  startCountdownTicker();
  renderContinueWatching();
  renderMyList();
  renderNotifBadge();
  loadHeroContent();
  loadHomeSeasonal();
  checkNotifications();
  setInterval(checkNotifications, 30 * 60 * 1000);
});
