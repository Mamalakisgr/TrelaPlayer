import { listStore } from './storage.js';

const store = listStore('trela_watched');

export const getWatched = store.getAll;
export const isWatched = store.has;

// entry: { id, kind, title }
export const addToWatched = (entry) => store.upsert(entry, 'watchedAt');
export const removeFromWatched = store.remove;
