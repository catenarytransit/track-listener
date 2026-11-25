import test from 'node:test';
import assert from 'node:assert/strict';
import { createThemeController, normalizeTheme, resolveTheme, THEME_KEY } from './theme.js';

function environment(saved, dark = false, blocked = false) {
  let stored = saved;
  const events = new Map(), mediaEvents = new Map();
  const media = {
    matches: dark,
    addEventListener: (name, cb) => mediaEvents.set(name, cb),
    removeEventListener: name => mediaEvents.delete(name)
  };
  const meta = {};
  const doc = { documentElement: { dataset: {} }, querySelector: () => meta };
  const win = {
    localStorage: {
      getItem(key) { assert.equal(key, THEME_KEY); if (blocked) throw Error('blocked'); return stored; },
      setItem(key, value) { assert.equal(key, THEME_KEY); if (blocked) throw Error('blocked'); stored = value; }
    },
    matchMedia: () => media,
    addEventListener: (name, cb) => events.set(name, cb),
    removeEventListener: name => events.delete(name)
  };
  return {
    doc, win, meta,
    saved: () => stored,
    system(value) { media.matches = value; mediaEvents.get('change')?.(); },
    storage(key, newValue) { events.get('storage')?.({key,newValue}); },
    listeners: () => events.size + mediaEvents.size
  };
}

test('system and explicit theme resolution', () => {
  for (const invalid of [null, undefined, '', 'invalid']) assert.equal(normalizeTheme(invalid), 'system');
  assert.equal(resolveTheme('system', true), 'dark');
  assert.equal(resolveTheme('system', false), 'light');
  assert.equal(resolveTheme('dark', false), 'dark');
  assert.equal(resolveTheme('light', true), 'light');
});

test('restores preference, persists manual changes, follows system only in system mode', () => {
  const env = environment('dark');
  let selected;
  const controller = createThemeController(env.win, env.doc, value => { selected = value; });
  const active = () => env.doc.documentElement.dataset.theme;
  assert.equal(selected, 'dark');
  assert.equal(active(), 'dark');
  env.system(true);
  env.system(false);
  assert.equal(active(), 'dark');
  controller.set('light');
  assert.equal(env.saved(), 'light');
  env.system(true);
  assert.equal(active(), 'light');
  controller.set('system');
  assert.equal(env.saved(), 'system');
  assert.equal(active(), 'dark');
  assert.equal(env.meta.content, '#0c1719');
  env.system(false);
  assert.equal(active(), 'light');
  env.storage(THEME_KEY, 'dark');
  assert.equal(selected, 'dark');
  assert.equal(active(), 'dark');
  env.storage(null, null);
  assert.equal(selected, 'system');
  assert.equal(active(), 'light');
  controller.destroy();
  assert.equal(env.listeners(), 0);
});

test('storage failure does not prevent in-session selection', () => {
  const env = environment(null, true, true);
  const controller = createThemeController(env.win, env.doc);
  assert.equal(env.doc.documentElement.dataset.theme, 'dark');
  controller.set('light');
  assert.equal(env.doc.documentElement.dataset.theme, 'light');
  controller.destroy();
});
