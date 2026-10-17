import assert from 'node:assert/strict';
import test from 'node:test';

import {
  getSettings,
  undocumentedConfigStructs,
  undocumentedConfigFields,
  unknownDirectives,
  misplacedDirectives,
} from './settings.js';

function find(entries, name, direction) {
  return entries.find((entry) => entry.name === name && entry.direction === direction);
}

function rowsOf(entry, label) {
  const group = label ? entry.groups.find((g) => g.label === label) : entry.groups[0];
  return group.settings;
}

test('every config struct in the codebase is documented', () => {
  assert.deepEqual(undocumentedConfigStructs(), []);
});

test('every config field is shown as a row or marked internal', () => {
  assert.deepEqual(undocumentedConfigFields(), []);
});

test('every @docs directive key is recognized', () => {
  assert.deepEqual(unknownDirectives(), []);
});

test('every @docs directive is attached to an item', () => {
  assert.deepEqual(misplacedDirectives(), []);
});

test('NDI input shows the two UI rows with their option labels', () => {
  const ndi = find(getSettings(), 'NDI', 'input');
  const rows = rowsOf(ndi);
  assert.deepEqual(
    rows.map((row) => row.label),
    ['Bandwidth', 'Pixel Format'],
  );
  assert.deepEqual(rows[0].options, ['Highest', 'Lowest']);
  assert.deepEqual(rows[1].options, ['BGRX/BGRA', 'UYVY/BGRA', 'RGBX/RGBA', 'UYVY/RGBA']);
});

test('DeckLink input Connection lists the physical connections', () => {
  const decklink = find(getSettings(), 'DeckLink', 'input');
  const connection = rowsOf(decklink).find((row) => row.label === 'Connection');
  assert.ok(connection.options.includes('SDI'));
  assert.ok(connection.options.includes('HDMI'));
  assert.equal(connection.default, 'Unspecified');
});

test('grouped Mode rows use the inline desc directive', () => {
  const avf = find(getSettings(), 'AVFoundation', 'input');
  const mode = rowsOf(avf).find((row) => row.label === 'Mode');
  assert.equal(mode.type, 'select');
  assert.match(mode.doc, /resolution and frame rate/i);
});

test('internal fields are not shown', () => {
  const avf = find(getSettings(), 'AVFoundation', 'input');
  assert.ok(!rowsOf(avf).some((row) => /device_unique_id/.test(row.label)));
  const ndiOut = find(getSettings(), 'NDI', 'output');
  assert.equal(rowsOf(ndiOut).length, 0);
});

test('Test patterns lists the per-field rows', () => {
  const labels = rowsOf(find(getSettings(), 'Test patterns', 'input')).map((row) => row.label);
  assert.ok(labels.includes('Width'));
  assert.ok(labels.includes('Cursor speed X'));
  assert.ok(labels.includes('Pattern'));
});

test('every protocol/direction yields a group', () => {
  const entries = getSettings();
  assert.ok(entries.length >= 10);
  for (const entry of entries) {
    assert.equal(entry.groups.length > 0, true, `${entry.name} ${entry.direction} has groups`);
  }
});
