const STORAGE_KEY = 'trela_tmdb_api_key';

export function getTmdbKey() {
  return localStorage.getItem(STORAGE_KEY) || '';
}

export function setTmdbKey(key) {
  localStorage.setItem(STORAGE_KEY, key.trim());
}
