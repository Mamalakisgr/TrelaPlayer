export function debounce(fn, delayMs) {
  let timer;
  return (...args) => {
    clearTimeout(timer);
    timer = setTimeout(() => fn(...args), delayMs);
  };
}

export function escapeAttr(s) {
  return String(s).replace(/&/g, '&amp;').replace(/"/g, '&quot;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
}

// ---------- Next-episode countdowns ----------
//
// AniList gives a unix timestamp for the next episode of an airing show (or
// the announced premiere date of an upcoming one — see parse_next_airing in
// anilist.rs). Countdown elements are rendered with their text already
// correct and carry data-airing-at so one shared ticker can keep every
// countdown on the page fresh without a timer per card.

// Coarse by design: a countdown a day out doesn't need its minutes ticking,
// and the shared ticker only refreshes once a minute anyway.
export function formatCountdown(secondsRemaining) {
  if (secondsRemaining < 60) return 'Airing now';
  const days = Math.floor(secondsRemaining / 86400);
  const hours = Math.floor((secondsRemaining % 86400) / 3600);
  const minutes = Math.floor((secondsRemaining % 3600) / 60);
  if (days) return hours ? `${days}d ${hours}h` : `${days}d`;
  if (hours) return minutes ? `${hours}h ${minutes}m` : `${hours}h`;
  return `${minutes}m`;
}

function countdownText(airingAt, episode, format) {
  const remaining = airingAt - Math.floor(Date.now() / 1000);
  const time = formatCountdown(remaining);
  if (format !== 'full') return time;
  if (remaining < 60) return episode ? `Episode ${episode} airing now` : 'Premiering now';
  return episode ? `Episode ${episode} airs in ${time}` : `Premieres in ${time}`;
}

function countdownAttrs(item, format) {
  return `data-airing-at="${item.next_airing_at}" data-countdown-format="${format}"${item.next_episode ? ` data-episode="${item.next_episode}"` : ''}`;
}

// Compact pill for poster cards — sits in the always-visible layer opposite
// the score badge, since a countdown you have to hover to see defeats the
// point of showing it while browsing.
export function countdownBadgeHtml(item) {
  if (!item.next_airing_at) return '';
  return `<span class="poster-countdown" ${countdownAttrs(item, 'compact')}>${countdownText(item.next_airing_at, item.next_episode, 'compact')}</span>`;
}

// Verbose form for detail pages, where there's room to name the episode.
export function countdownLineHtml(item) {
  if (!item.next_airing_at) return '';
  return `<p class="detail-countdown" ${countdownAttrs(item, 'full')}>${countdownText(item.next_airing_at, item.next_episode, 'full')}</p>`;
}

// One interval for the whole app rather than one per card: cards are
// re-rendered constantly (slideshow paging, search) and per-card timers
// would leak on every re-render. Elements drop their data-airing-at once
// they fire, so a finished countdown stops being revisited.
export function startCountdownTicker() {
  setInterval(() => {
    document.querySelectorAll('[data-airing-at]').forEach((el) => {
      const airingAt = Number(el.dataset.airingAt);
      el.textContent = countdownText(airingAt, el.dataset.episode, el.dataset.countdownFormat);
      if (airingAt - Math.floor(Date.now() / 1000) < 60) el.removeAttribute('data-airing-at');
    });
  }, 60000);
}

// Sized by a CSS grid parent (like .poster-grid) — for search-result grids.
export function skeletonGrid(count = 10) {
  return Array.from({ length: count }, () => `
    <div class="skeleton-card">
      <div class="skeleton-poster"></div>
      <div class="skeleton-line"></div>
      <div class="skeleton-line short"></div>
    </div>
  `).join('');
}

// Same cards as skeletonGrid, wrapped for a fixed 5-wide flex row instead.
export function skeletonRow(count = 5) {
  return `<div class="skeleton-row">${skeletonGrid(count)}</div>`;
}

// Matches the .hero shape the real renderSpotlightCard() fills in (see
// js/main.js) — containerEl already carries class="hero", so this just needs
// to fill it and echo the tag/title/desc it's about to be replaced with.
export function skeletonSpotlight() {
  return `
    <div class="skeleton-poster"></div>
    <div class="hero-content">
      <div class="skeleton-line short"></div>
      <div class="skeleton-line lg"></div>
      <div class="skeleton-line"></div>
    </div>
  `;
}

// Renders an inline error with a Retry button (AniList/the scraped anime
// provider both occasionally 5xx transiently — retrying is more useful than
// a dead end).
export function renderError(container, message, onRetry) {
  container.innerHTML = `
    <div class="error-box">
      <p>${escapeAttr(message)}</p>
      <button type="button" class="retry-btn">Retry</button>
    </div>
  `;
  container.querySelector('.retry-btn').addEventListener('click', onRetry);
}
