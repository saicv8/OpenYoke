// Display preferences: colour theme, text size, and whether the sidebar is open.
//
// Loaded synchronously from <head> — before the body renders — so every one of
// these is already on <html> at first paint and the window never flashes the
// wrong theme, the wrong text size, or an open sidebar that then snaps shut.
// They live in localStorage rather than the settings file because they're
// per-machine display choices, and because they have to be readable before the
// Tauri backend is reachable.
(function () {
  const THEME_KEY = 'openyoke.theme';
  const FONT_KEY = 'openyoke.fontScale';
  const SIDEBAR_KEY = 'openyoke.sidebar';

  const THEMES = ['system', 'light', 'dark'];
  const FONT_SCALES = ['0.9', '1', '1.15', '1.3']; // must match index.html's <select>
  const darkQuery = window.matchMedia('(prefers-color-scheme: dark)');
  const root = document.documentElement;

  // localStorage can throw in restricted webview contexts. A missing or bogus
  // preference just means "use the default", so never let it break boot.
  function read(key, allowed, fallback) {
    try {
      const saved = localStorage.getItem(key);
      return allowed.includes(saved) ? saved : fallback;
    } catch (error) {
      return fallback;
    }
  }

  function write(key, value) {
    try {
      localStorage.setItem(key, value);
    } catch (error) {
      /* the preference just won't survive a restart */
    }
  }

  let theme = read(THEME_KEY, THEMES, 'system');
  let fontScale = read(FONT_KEY, FONT_SCALES, '1');
  let sidebar = read(SIDEBAR_KEY, ['expanded', 'collapsed'], 'expanded');

  // --- Applying ------------------------------------------------------------

  function applyTheme() {
    const resolved = theme === 'system' ? (darkQuery.matches ? 'dark' : 'light') : theme;
    root.setAttribute('data-theme', resolved);
  }

  function applyFontScale() {
    root.style.setProperty('--font-scale', fontScale);
  }

  function applySidebar() {
    root.setAttribute('data-sidebar', sidebar);
    const toggle = document.getElementById('sidebar-toggle');
    if (!toggle) return;
    const open = sidebar !== 'collapsed';
    toggle.setAttribute('aria-expanded', String(open));
    toggle.title = open ? 'Hide sidebar (⌘B)' : 'Show sidebar (⌘B)';
  }

  applyTheme();
  applyFontScale();
  applySidebar();

  // --- Changing ------------------------------------------------------------

  function setTheme(next) {
    if (!THEMES.includes(next)) return;
    theme = next;
    write(THEME_KEY, next);
    applyTheme();
    syncThemeButtons();
  }

  function setFontScale(next) {
    if (!FONT_SCALES.includes(next)) return;
    fontScale = next;
    write(FONT_KEY, next);
    applyFontScale();
  }

  function setSidebar(next) {
    sidebar = next === 'collapsed' ? 'collapsed' : 'expanded';
    write(SIDEBAR_KEY, sidebar);
    applySidebar();
  }

  function toggleSidebar() {
    setSidebar(sidebar === 'collapsed' ? 'expanded' : 'collapsed');
  }

  function syncThemeButtons() {
    document.querySelectorAll('[data-theme-choice]').forEach((button) => {
      button.setAttribute('aria-pressed', String(button.dataset.themeChoice === theme));
    });
  }

  // "System" keeps tracking the OS while the app is open.
  darkQuery.addEventListener('change', () => {
    if (theme === 'system') applyTheme();
  });

  // The controls live in the body, so wire them once it exists.
  document.addEventListener('DOMContentLoaded', () => {
    syncThemeButtons();
    document.querySelectorAll('[data-theme-choice]').forEach((button) => {
      button.addEventListener('click', () => setTheme(button.dataset.themeChoice));
    });

    const fontSelect = document.getElementById('font-size');
    if (fontSelect) {
      fontSelect.value = fontScale;
      fontSelect.addEventListener('change', () => setFontScale(fontSelect.value));
    }

    const toggle = document.getElementById('sidebar-toggle');
    if (toggle) toggle.addEventListener('click', toggleSidebar);
    applySidebar(); // now that the button exists, label it
  });

  // ⌘B / Ctrl+B, the usual shortcut for this.
  document.addEventListener('keydown', (event) => {
    if ((event.metaKey || event.ctrlKey) && !event.altKey && event.key.toLowerCase() === 'b') {
      event.preventDefault();
      toggleSidebar();
    }
  });

  window.OpenYokeAppearance = {
    get theme() {
      return theme;
    },
    get fontScale() {
      return fontScale;
    },
    get sidebar() {
      return sidebar;
    },
    setTheme,
    setFontScale,
    setSidebar,
    toggleSidebar,
  };
})();
