import assert from 'node:assert/strict';
import test from 'node:test';

import { readRust } from './rust-source.js';
import { getOsc, oscRouteCount, oscRouteTripwire } from './osc.js';

const FILES = ['src/control/osc/server.rs', 'src/control/osc/feedback.rs'];
const INPUT_ORDER = ['Sources', 'Placement', 'Canvas & view', 'Outputs', 'Project'];
const FEEDBACK_ORDER = ['Global', 'Sources', 'Outputs'];

function flat(kind) {
  return getOsc()[kind].flatMap((group) => group.routes);
}

test('lists every route with a path and description', () => {
  const routes = [...flat('input'), ...flat('feedback')];
  assert.equal(routes.length, oscRouteTripwire());
  for (const route of routes) {
    assert.ok(route.path.startsWith('/'), `bad path: ${route.path}`);
    assert.ok(route.desc.length > 0, `missing description for ${route.path}`);
  }
});

test('input paths are unique', () => {
  const paths = flat('input').map((route) => route.path);
  assert.equal(new Set(paths).size, paths.length);
});

test('feedback paths are unique', () => {
  const paths = flat('feedback').map((route) => route.path);
  assert.equal(new Set(paths).size, paths.length);
});

test('route count matches the Rust tripwire', () => {
  assert.equal(oscRouteCount(), oscRouteTripwire());
});

test('groups are in the documented order', () => {
  const { input, feedback } = getOsc();
  assert.deepEqual(
    input.map((group) => group.group),
    INPUT_ORDER,
  );
  assert.deepEqual(
    feedback.map((group) => group.group),
    FEEDBACK_ORDER,
  );
});

test('feedback routes use the feedback kind', () => {
  for (const route of flat('feedback')) {
    assert.equal(route.kind, 'feedback', `wrong kind for ${route.path}`);
  }
});

test('directives use known keys', () => {
  const known = new Set(['path', 'args', 'group', 'desc', 'kind']);
  for (const file of FILES) {
    const text = readRust(file);
    assert.ok(text, `${file} is missing`);
    for (const raw of text.split('\n')) {
      const line = raw.trim();
      if (!line.startsWith('// @osc ')) continue;
      for (const match of line.matchAll(/(\w+)\s*=\s*"([^"]*)"/g)) {
        assert.ok(known.has(match[1]), `unknown directive key: ${match[1]}`);
      }
    }
  }
});
