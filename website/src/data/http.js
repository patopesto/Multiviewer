// HTTP routes, read from the `// @http` directives in the API router at Astro
// build time so the docs page matches the code and cannot drift.
import { readRust } from './rust-source.js';

const FILE = 'src/control/http/api.rs';
const GROUP_ORDER = ['State', 'Project', 'Commands', 'Web UI'];

function parseDirective(text) {
  const out = {};
  for (const match of text.matchAll(/(\w+)\s*=\s*"([^"]*)"/g)) out[match[1]] = match[2];
  return out;
}

function rank(group) {
  const index = GROUP_ORDER.indexOf(group);
  return index === -1 ? GROUP_ORDER.length : index;
}

/**
 * @typedef {object} HttpRoute
 * @property {string} method
 * @property {string} path
 * @property {string} desc
 * @property {string} group
 */

/** HTTP routes grouped for the docs table. */
export function getHttp() {
  const routes = [];
  const text = readRust(FILE) ?? '';
  for (const raw of text.split('\n')) {
    const line = raw.trim();
    if (!line.startsWith('// @http ')) continue;
    const directive = parseDirective(line.slice(8));
    routes.push({
      method: directive.method ?? 'GET',
      path: directive.path ?? '',
      desc: directive.desc ?? '',
      group: directive.group ?? 'Commands',
    });
  }

  const groups = [];
  for (const route of routes) {
    let entry = groups.find((candidate) => candidate.group === route.group);
    if (!entry) {
      entry = { group: route.group, routes: [] };
      groups.push(entry);
    }
    entry.routes.push(route);
  }
  return groups.sort((a, b) => rank(a.group) - rank(b.group));
}

/** Number of documented HTTP routes. */
export function httpRouteCount() {
  return getHttp().reduce((count, group) => count + group.routes.length, 0);
}

/** The `HTTP_ROUTE_COUNT` tripwire from the Rust source, or null when absent. */
export function httpRouteTripwire() {
  const text = readRust(FILE);
  const match = text && /HTTP_ROUTE_COUNT:\s*usize\s*=\s*(\d+)/.exec(text);
  return match ? Number(match[1]) : null;
}
