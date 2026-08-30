export function readJson(key, fallback) {
  try {
    const parsed = JSON.parse(localStorage.getItem(key));
    return parsed ?? fallback;
  } catch {
    return fallback;
  }
}

// Shared shape behind watched/wishlist/continueWatching: a list of entries
// keyed by id, newest first, persisted as one JSON blob under `key`.
export function listStore(key, { max = Infinity } = {}) {
  const getAll = () => {
    const list = readJson(key, []);
    return Array.isArray(list) ? list : [];
  };
  const save = (list) => localStorage.setItem(key, JSON.stringify(list.slice(0, max)));
  return {
    getAll,
    has: (id) => getAll().some((e) => e.id === id),
    upsert: (entry, tsField) => save([{ ...entry, [tsField]: Date.now() }, ...getAll().filter((e) => e.id !== entry.id)]),
    remove: (id) => save(getAll().filter((e) => e.id !== id)),
  };
}
