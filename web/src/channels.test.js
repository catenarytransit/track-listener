import { test } from 'node:test';
import assert from 'node:assert/strict';
import { groupChannels } from './channels.js';

test('groups channels by region without changing channel or region order', () => {
  const a = { id: 'orange', region: 'California' };
  const b = { id: 'zurich', region: 'Switzerland' };
  const c = { id: 'caltrain', region: ' California ' };
  assert.deepEqual(groupChannels([a, b, c]), [
    { name: 'California', channels: [a, c] },
    { name: 'Switzerland', channels: [b] },
  ]);
});

test('empty and missing regions are grouped under Unassigned', () => {
  const a = { id: 'a' };
  const b = { id: 'b', region: '   ' };
  assert.deepEqual(groupChannels([a, b]), [{ name: 'Unassigned', channels: [a, b] }]);
  assert.deepEqual(groupChannels([]), []);
});
