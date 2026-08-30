import { listStore } from './storage.js';

const store = listStore('trela_wishlist');

export const getWishlist = store.getAll;
export const isInWishlist = store.has;

// entry: { id, kind, title, year, image_url, tmdb_id }
export const addToWishlist = (entry) => store.upsert(entry, 'addedAt');
export const removeFromWishlist = store.remove;
