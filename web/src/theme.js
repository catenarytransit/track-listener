export const THEME_KEY = 'track-listener.theme';

export function normalizeTheme(value) {
  return value === 'light' || value === 'dark' ? value : 'system';
}

export function resolveTheme(preference, prefersDark) {
  const normalized = normalizeTheme(preference);
  return normalized === 'system' ? (prefersDark ? 'dark' : 'light') : normalized;
}

export function createThemeController(win, doc, onChange = () => {}) {
  const media = win.matchMedia('(prefers-color-scheme: dark)');
  let preference = 'system';
  try { preference = normalizeTheme(win.localStorage.getItem(THEME_KEY)); } catch (_) {}

  function apply() {
    const theme = resolveTheme(preference, media.matches);
    doc.documentElement.dataset.theme = theme;
    const meta = doc.querySelector('meta[name="theme-color"]');
    if (meta) meta.content = theme === 'dark' ? '#202124' : '#f4f4f4';
    onChange(preference);
  }

  function set(value) {
    preference = normalizeTheme(value);
    try { win.localStorage.setItem(THEME_KEY, preference); } catch (_) {}
    apply();
  }

  function onStorage(event) {
    if (event.key !== THEME_KEY && event.key !== null) return;
    preference = normalizeTheme(event.newValue);
    apply();
  }

  function onSystemChange() {
    if (preference === 'system') apply();
  }

  media.addEventListener('change', onSystemChange);
  win.addEventListener('storage', onStorage);
  apply();

  return {
    set,
    destroy() {
      media.removeEventListener('change', onSystemChange);
      win.removeEventListener('storage', onStorage);
    }
  };
}
