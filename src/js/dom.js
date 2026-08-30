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
