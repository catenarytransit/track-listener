import test from 'node:test';
import assert from 'node:assert/strict';
import { chronologicalPage, prependOlderClips, scrollIntent } from './timeline.js';

const clip = (id, started_ms, sentences = []) => ({ id, started_ms, sentences });
const a = clip('a', 100), b = clip('b', 100), c = clip('c', 200, [{ordinal: 0}, {ordinal: 1}]);

test('latest API page is displayed oldest first without reversing sentences or mutating data', () => {
  const page = [c, b, a];
  assert.deepEqual(chronologicalPage(page), [a, b, c]);
  assert.deepEqual(page, [c, b, a]);
  assert.deepEqual(c.sentences.map(s => s.ordinal), [0, 1]);
});

test('older pages prepend in chronological order and skip overlapping IDs', () => {
  const d = clip('d', 20), e = clip('e', 30);
  assert.deepEqual(prependOlderClips([b, c], [b, a, e, d]), [d, e, a, b, c]);
  assert.deepEqual(prependOlderClips([b, c], []), [b, c]);
  assert.deepEqual(prependOlderClips([b, c], [a, a]), [a, b, c]);
});

test('upward scroll away from the latest pauses following; near the top requests history', () => {
  assert.deepEqual(scrollIntent(500, {scrollTop: 400, scrollHeight: 1000, clientHeight: 500}), {browse: true, older: false});
  assert.deepEqual(scrollIntent(100, {scrollTop: 50, scrollHeight: 1000, clientHeight: 500}), {browse: true, older: true});
  assert.deepEqual(scrollIntent(50, {scrollTop: 50, scrollHeight: 1000, clientHeight: 500}), {browse: false, older: false});
  assert.deepEqual(scrollIntent(50, {scrollTop: 100, scrollHeight: 1000, clientHeight: 500}), {browse: false, older: false});
  assert.deepEqual(scrollIntent(500, {scrollTop: 480, scrollHeight: 1000, clientHeight: 500}), {browse: false, older: false});
});
