import { readJson } from './storage.js';

const STORAGE_KEY = 'trela_watched';

export function getWatched() {
  const list = readJson(STORAGE_KEY, []);
  return Array.isArray(list) ? list : [];
}

export function isWatched(id) {
  return getWatched().some((e) => e.id === id);
}

// entry: { id, kind, title }
export function addToWatched(entry) {
  const list = getWatched().filter((e) => e.id !== entry.id);
  list.unshift({ ...entry, watchedAt: Date.now() });
  localStorage.setItem(STORAGE_KEY, JSON.stringify(list));
}

export function removeFromWatched(id) {
  localStorage.setItem(STORAGE_KEY, JSON.stringify(getWatched().filter((e) => e.id !== id)));
}
