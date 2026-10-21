// OSC command paths, read from the `// @osc` directives in the OSC server at
// Astro build time so the docs page matches the parser and cannot drift.
import { readRust } from './rust-source.js';

const GROUP_ORDER = ['Sources', 'Placement', 'Canvas & view', 'Outputs', 'Project'];

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
 * @typedef {object} OscRoute
 * @property {string} path
 * @property {string} args
 * @property {string} desc
 *
 * @typedef {object} OscGroup
 * @property {string} group
 * @property {OscRoute[]} routes
 */

/**
 * OSC routes grouped for the docs table, in `GROUP_ORDER`.
 * @returns {OscGroup[]}
 */
export function getOsc() {
  const text = readRust('src/control/osc/server.rs');
  if (!text) return [];
  const groups = [];
  for (const raw of text.split('\n')) {
    const line = raw.trim();
    if (!line.startsWith('// @osc ')) continue;
    const directive = parseDirective(line.slice(8));
    const group = directive.group ?? 'Commands';
    let entry = groups.find((candidate) => candidate.group === group);
    if (!entry) {
      entry = { group, routes: [] };
      groups.push(entry);
    }
    entry.routes.push({
      path: directive.path ?? '',
      args: directive.args ?? '',
      desc: directive.desc ?? '',
    });
  }
  return groups.sort((a, b) => rank(a.group) - rank(b.group));
}

/** Number of documented OSC routes. */
export function oscRouteCount() {
  return getOsc().reduce((count, group) => count + group.routes.length, 0);
}

/** The `OSC_ROUTE_COUNT` tripwire from the Rust source, or null when absent. */
export function oscRouteTripwire() {
  const text = readRust('src/control/osc/server.rs');
  const match = text && /OSC_ROUTE_COUNT:\s*usize\s*=\s*(\d+)/.exec(text);
  return match ? Number(match[1]) : null;
}
