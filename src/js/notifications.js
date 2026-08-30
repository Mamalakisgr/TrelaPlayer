import { readJson } from './storage.js';

const COUNTS_KEY = 'trela_notif_counts'; // { [id]: lastKnownEpisodeCount }
const LIST_KEY = 'trela_notifications'; // [{ id, itemId, kind, title, message, createdAt, read }]
const MAX_NOTIFICATIONS = 30;

export function getNotifications() {
  const list = readJson(LIST_KEY, []);
  return Array.isArray(list) ? list : [];
}

export function unreadCount() {
  return getNotifications().filter((n) => !n.read).length;
}

export function markAllRead() {
  localStorage.setItem(LIST_KEY, JSON.stringify(getNotifications().map((n) => ({ ...n, read: true }))));
}

export function clearNotifications() {
  localStorage.setItem(LIST_KEY, JSON.stringify([]));
}

// items: [{ id, kind, title, count }] — count is the tracked item's current
// total episode count for anime/series, or 0/1 (not yet released/released)
// for a wishlisted movie (null/undefined if it couldn't be resolved this
// round, e.g. no TMDB key or a lookup failure; such items are just skipped).
// The first time an item is seen its count is only recorded as the
// baseline, not reported — every episode it already had (or a movie that
// was already out when wishlisted) isn't "new".
export function recordEpisodeCounts(items) {
  const knownCounts = readJson(COUNTS_KEY, {});
  const notifications = getNotifications();
  let changed = false;

  for (const item of items) {
    if (item.count == null) continue;
    const known = knownCounts[item.id];
    if (known !== undefined && item.count > known) {
      const delta = item.count - known;
      const message = item.kind === 'movie' ? 'Now available to watch' : `${delta} new episode${delta > 1 ? 's' : ''} available`;
      notifications.unshift({
        id: `${item.id}:${item.count}`,
        itemId: item.id,
        kind: item.kind,
        title: item.title,
        message,
        createdAt: Date.now(),
        read: false,
      });
      changed = true;
    }
    if (known !== item.count) {
      knownCounts[item.id] = item.count;
      changed = true;
    }
  }

  if (changed) {
    localStorage.setItem(COUNTS_KEY, JSON.stringify(knownCounts));
    localStorage.setItem(LIST_KEY, JSON.stringify(notifications.slice(0, MAX_NOTIFICATIONS)));
  }
}
