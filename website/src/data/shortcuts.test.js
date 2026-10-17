import assert from 'node:assert/strict';
import test from 'node:test';

import { getShortcuts } from './shortcuts.js';

function flat() {
  return getShortcuts().flatMap((group) => group.shortcuts);
}

test('lists every shortcut with a label and a key', () => {
  const shortcuts = flat();
  // Mirrors the `all_global_lists_every_shortcut` tripwire in shortcuts.rs.
  assert.equal(shortcuts.length, 18);
  for (const shortcut of shortcuts) {
    assert.ok(shortcut.label.length > 0, 'missing label');
    assert.ok(shortcut.key.length > 0, `missing key for ${shortcut.label}`);
  }
});

test('groups by category in all_global order', () => {
  assert.deepEqual(
    getShortcuts().map((group) => group.category),
    ['Project', 'Sources', 'Canvas'],
  );
});

test('renders platform-correct and literal keys', () => {
  const shortcuts = flat();
  assert.equal(shortcuts.find((s) => s.label === 'New Project').key, 'Cmd/Ctrl + N');
  assert.equal(shortcuts.find((s) => s.label === 'Delete Source').key, 'Delete');
  assert.equal(shortcuts.find((s) => s.label === 'Hide UI').key, 'Space');
});

test('nudge rows carry the fast-nudge detail', () => {
  const shortcuts = flat();
  assert.match(shortcuts.find((s) => s.label === 'Nudge Source Up').detail, /10 px/);
});
