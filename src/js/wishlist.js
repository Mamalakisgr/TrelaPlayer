import { readJson } from './storage.js';

const STORAGE_KEY = 'trela_wishlist';

export function getWishlist() {
  const list = readJson(STORAGE_KEY, []);
  return Array.isArray(list) ? list : [];
}

export function isInWishlist(id) {
  return getWishlist().some((e) => e.id === id);
}

// entry: { id, kind, title, year, image_url, tmdb_id }
export function addToWishlist(entry) {
  const list = getWishlist().filter((e) => e.id !== entry.id);
  list.unshift({ ...entry, addedAt: Date.now() });
  localStorage.setItem(STORAGE_KEY, JSON.stringify(list));
}

export function removeFromWishlist(id) {
  localStorage.setItem(STORAGE_KEY, JSON.stringify(getWishlist().filter((e) => e.id !== id)));
}
