import { listStore } from './storage.js';

const store = listStore('trela_continue_watching', { max: 10 });

export const getContinueWatching = store.getAll;

// entry: { id, kind: 'anime'|'movie'|'series', title, episode, season, quality, year, image_url }
// (season/quality only apply to some kinds; missing `kind` on old stored entries means 'anime';
// entries saved before image_url existed just render without a poster)
export const saveContinueWatching = (entry) => store.upsert(entry, 'updatedAt');
export const removeContinueWatching = store.remove;

// Called when mpv reports how far playback actually got (see the
// playback-progress event in main.js) — merges into the existing entry
// without touching its other fields, and no-ops if the entry was removed
// (e.g. the user cleared it) while still watching.
export function updateContinueWatchingProgress(id, positionSeconds, durationSeconds) {
  const existing = getContinueWatching().find((e) => e.id === id);
  if (!existing) return;
  store.upsert({ ...existing, positionSeconds, durationSeconds }, 'updatedAt');
}
