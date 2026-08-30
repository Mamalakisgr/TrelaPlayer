import { escapeAttr, skeletonRow } from './dom.js';

const PAGE_SIZE = 5;

// A fixed-page-size poster carousel. .poster-card is sized in CSS to fill
// exactly 1/PAGE_SIZE of the track, and justify-content:center centers a
// short last page — pageing swaps the rendered DOM rather than scrolling,
// so pages never land mid-card.
export function createSlideshow(trackEl, { onSelect }) {
  let items = [];
  let page = 0;
  let renderCard = () => '';

  function render() {
    if (!items.length) {
      trackEl.innerHTML = '<p class="muted-center">Nothing found</p>';
      return;
    }
    const totalPages = Math.ceil(items.length / PAGE_SIZE);
    page = ((page % totalPages) + totalPages) % totalPages;
    const start = page * PAGE_SIZE;
    const pageItems = items.slice(start, start + PAGE_SIZE);

    trackEl.innerHTML = pageItems
      .map((item, i) => `<div class="poster-card" data-index="${start + i}" tabindex="0" role="button">${renderCard(item)}</div>`)
      .join('');

    trackEl.querySelectorAll('.poster-card').forEach((card) => {
      card.addEventListener('click', () => onSelect(items[Number(card.dataset.index)]));
    });
  }

  return {
    loading() {
      trackEl.innerHTML = skeletonRow(PAGE_SIZE);
    },
    setItems(newItems, cardRenderer) {
      items = newItems;
      renderCard = cardRenderer;
      page = 0;
      render();
    },
    goToPage(dir) {
      page += dir;
      render();
    },
  };
}

// Shared skeleton for both anime and movie/series poster cards — only the
// subtitle parts (episode count vs year, etc.) differ per data shape.
// Bare poster at rest; title/sub/genres sit in a bottom-scrim overlay that
// styles.css only shows on hover/focus (.poster-card .poster-reveal) — the
// rating badge is the one thing that stays visible always.
function posterCardHtml(item, subParts) {
  const score = item.score || item.rating;
  const genres = (item.genres || [])
    .slice(0, 2)
    .map((g) => `<span class="tag tag-outline">${escapeAttr(g)}</span>`)
    .join('');

  return `
    <div class="poster-image-wrap">
      <img src="${item.image_url}" alt="${escapeAttr(item.title)}" loading="lazy" />
      ${score ? `<span class="poster-score">★ ${item.rating ? score.toFixed(1) : score}</span>` : ''}
      <div class="poster-reveal">
        <div class="poster-title">${item.title}</div>
        <div class="poster-sub">${subParts.join(' · ')}</div>
        ${genres ? `<div class="genre-chips">${genres}</div>` : ''}
      </div>
    </div>
  `;
}

export function seasonalCardHtml(item) {
  const sub = [];
  if (item.episodes) sub.push(`${item.episodes} eps`);
  else if (item.status) sub.push(item.status);
  if (item.score) sub.push(`★ ${item.score}`);
  return posterCardHtml(item, sub);
}

export function movieCardHtml(item) {
  const sub = [];
  if (item.year) sub.push(item.year);
  if (item.rating) sub.push(`★ ${item.rating.toFixed(1)}`);
  return posterCardHtml(item, sub);
}

export function recentEpisodeCardHtml(item) {
  return `
    <div class="poster-image-wrap">
      <img src="${item.image_url}" alt="${escapeAttr(item.title)}" loading="lazy" />
      <div class="poster-reveal">
        <div class="poster-title">${item.title}</div>
        <div class="poster-sub">${escapeAttr(item.episode_title)}</div>
      </div>
    </div>
  `;
}
