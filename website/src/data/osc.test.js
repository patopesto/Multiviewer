import assert from 'node:assert/strict';
import test from 'node:test';

import { readRust } from './rust-source.js';
import { getOsc, oscRouteCount, oscRouteTripwire } from './osc.js';

function flat() {
  return getOsc().flatMap((group) => group.routes);
}

test('lists every route with a path and description', () => {
  const routes = flat();
  assert.equal(routes.length, oscRouteTripwire());
  for (const route of routes) {
    assert.ok(route.path.startsWith('/'), `bad path: ${route.path}`);
    assert.ok(route.desc.length > 0, `missing description for ${route.path}`);
  }
});

test('route paths are unique', () => {
  const paths = flat().map((route) => route.path);
  assert.equal(new Set(paths).size, paths.length);
});

test('route count matches the Rust tripwire', () => {
  assert.equal(oscRouteCount(), oscRouteTripwire());
});

test('groups are in the documented order', () => {
  assert.deepEqual(
    getOsc().map((group) => group.group),
    ['Sources', 'Placement', 'Canvas & view', 'Outputs', 'Project'],
  );
});

test('directives use known keys', () => {
  const text = readRust('src/control/osc/server.rs');
  assert.ok(text, 'OSC server source is missing');
  const known = new Set(['path', 'args', 'group', 'desc']);
  for (const raw of text.split('\n')) {
    const line = raw.trim();
    if (!line.startsWith('// @osc ')) continue;
    for (const match of line.matchAll(/(\w+)\s*=\s*"([^"]*)"/g)) {
      assert.ok(known.has(match[1]), `unknown directive key: ${match[1]}`);
    }
  }
});
