const ARCHIVE_INTERVAL_KEY = "qzone-archive-page-interval";
const AUTO_UPDATE_KEY = "qzone-auto-update";
const MEDIA_LAZY_LOAD_KEY = "qzone-media-lazy-load";
export const MIN_ARCHIVE_INTERVAL = 2000;
export const DEFAULT_ARCHIVE_INTERVAL = 3000;
export const DEFAULT_AUTO_UPDATE = true;
export const DEFAULT_MEDIA_LAZY_LOAD = true;

export function getArchiveInterval() {
  const value = Number(localStorage.getItem(ARCHIVE_INTERVAL_KEY));
  return Number.isFinite(value) ? Math.min(30000, Math.max(MIN_ARCHIVE_INTERVAL, Math.round(value))) : DEFAULT_ARCHIVE_INTERVAL;
}

export function setArchiveInterval(value: number) {
  const normalized = Math.min(30000, Math.max(MIN_ARCHIVE_INTERVAL, Math.round(value || DEFAULT_ARCHIVE_INTERVAL)));
  localStorage.setItem(ARCHIVE_INTERVAL_KEY, String(normalized));
  return normalized;
}

export function getAutoUpdate() {
  const stored = localStorage.getItem(AUTO_UPDATE_KEY);
  return stored === null ? DEFAULT_AUTO_UPDATE : stored === "true";
}

export function setAutoUpdate(value: boolean) {
  localStorage.setItem(AUTO_UPDATE_KEY, String(value));
  return value;
}

export function getMediaLazyLoad() {
  const stored = localStorage.getItem(MEDIA_LAZY_LOAD_KEY);
  return stored === null ? DEFAULT_MEDIA_LAZY_LOAD : stored === "true";
}

export function setMediaLazyLoad(value: boolean) {
  localStorage.setItem(MEDIA_LAZY_LOAD_KEY, String(value));
  return value;
}

export function resetAppSettings() {
  localStorage.removeItem(ARCHIVE_INTERVAL_KEY);
  localStorage.removeItem(AUTO_UPDATE_KEY);
  localStorage.removeItem(MEDIA_LAZY_LOAD_KEY);
}
