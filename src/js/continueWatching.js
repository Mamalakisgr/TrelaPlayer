import { readJson } from './storage.js';

const STORAGE_KEY = 'trela_continue_watching';
const MAX_ENTRIES = 10;

export function getContinueWatching() {
  const list = readJson(STORAGE_KEY, []);
  return Array.isArray(list) ? list : [];
}

// entry: { id, kind: 'anime'|'movie'|'series', title, episode, season, quality, year, image_url }
// (season/quality only apply to some kinds; missing `kind` on old stored entries means 'anime';
// entries saved before image_url existed just render without a poster)
export function saveContinueWatching(entry) {
  const list = getContinueWatching().filter((e) => e.id !== entry.id);
  list.unshift({ ...entry, updatedAt: Date.now() });
  localStorage.setItem(STORAGE_KEY, JSON.stringify(list.slice(0, MAX_ENTRIES)));
}

export function removeContinueWatching(id) {
  const list = getContinueWatching().filter((e) => e.id !== id);
  localStorage.setItem(STORAGE_KEY, JSON.stringify(list));
}
