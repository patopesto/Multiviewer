import assert from 'node:assert/strict';
import test from 'node:test';

import { readRust } from './rust-source.js';
import { getHttp, httpRouteCount, httpRouteTripwire } from './http.js';

const FILE = 'src/control/http/api.rs';
const GROUPS = ['State', 'Project', 'Commands', 'Web UI'];

test('lists every route with a method, path and description', () => {
  const routes = getHttp().flatMap((group) => group.routes);
  assert.equal(routes.length, httpRouteTripwire());
  for (const route of routes) {
    assert.match(route.method, /^(GET|PUT|POST|DELETE)$/);
    assert.ok(route.path.startsWith('/'), `bad path: ${route.path}`);
    assert.ok(route.desc.length > 0, `missing description for ${route.path}`);
  }
});

test('routes are unique per method and path', () => {
  const routes = getHttp().flatMap((group) => group.routes);
  const keys = routes.map((route) => `${route.method} ${route.path}`);
  assert.equal(new Set(keys).size, keys.length);
});

test('route count matches the Rust tripwire', () => {
  assert.equal(httpRouteCount(), httpRouteTripwire());
});

test('groups are in the documented order', () => {
  assert.deepEqual(
    getHttp().map((group) => group.group),
    GROUPS,
  );
});

test('directives use known keys', () => {
  const known = new Set(['method', 'path', 'group', 'desc']);
  const text = readRust(FILE);
  assert.ok(text, `${FILE} is missing`);
  for (const raw of text.split('\n')) {
    const line = raw.trim();
    if (!line.startsWith('// @http ')) continue;
    for (const match of line.matchAll(/(\w+)\s*=\s*"([^"]*)"/g)) {
      assert.ok(known.has(match[1]), `unknown directive key: ${match[1]}`);
    }
  }
});
