// OSC routes, read from the `// @osc` directives in the OSC server and feedback
// modules at Astro build time so the docs page matches the code and cannot drift.
import { readRust } from './rust-source.js';

const INPUT_ORDER = ['Sources', 'Placement', 'Canvas & view', 'Outputs', 'Project'];
const FEEDBACK_ORDER = ['Global', 'Sources', 'Outputs'];

const FILES = ['src/control/osc/server.rs', 'src/control/osc/feedback.rs'];

function parseDirective(text) {
  const out = {};
  for (const match of text.matchAll(/(\w+)\s*=\s*"([^"]*)"/g)) out[match[1]] = match[2];
  return out;
}

function rank(group, order) {
  const index = order.indexOf(group);
  return index === -1 ? order.length : index;
}

function groupRoutes(routes, order) {
  const groups = [];
  for (const route of routes) {
    let entry = groups.find((candidate) => candidate.group === route.group);
    if (!entry) {
      entry = { group: route.group, routes: [] };
      groups.push(entry);
    }
    entry.routes.push(route);
  }
  return groups.sort((a, b) => rank(a.group, order) - rank(b.group, order));
}

/**
 * @typedef {object} OscRoute
 * @property {string} path
 * @property {string} args
 * @property {string} desc
 * @property {string} kind
 *
 * @typedef {object} OscGroup
 * @property {string} group
 * @property {OscRoute[]} routes
 */

/**
 * OSC routes split into input commands and feedback output.
 * @returns {{ input: OscGroup[], feedback: OscGroup[] }}
 */
export function getOsc() {
  const routes = [];
  for (const file of FILES) {
    const text = readRust(file);
    if (!text) continue;
    for (const raw of text.split('\n')) {
      const line = raw.trim();
      if (!line.startsWith('// @osc ')) continue;
      const directive = parseDirective(line.slice(8));
      routes.push({
        path: directive.path ?? '',
        args: directive.args ?? '',
        desc: directive.desc ?? '',
        kind: directive.kind ?? 'command',
        group: directive.group ?? 'Commands',
      });
    }
  }
  return {
    input: groupRoutes(
      routes.filter((route) => route.kind === 'command'),
      INPUT_ORDER,
    ),
    feedback: groupRoutes(
      routes.filter((route) => route.kind === 'feedback'),
      FEEDBACK_ORDER,
    ),
  };
}

/** Number of documented OSC routes. */
export function oscRouteCount() {
  const { input, feedback } = getOsc();
  return [...input, ...feedback].reduce(
    (count, group) => count + group.routes.length,
    0,
  );
}

/** The `OSC_ROUTE_COUNT` tripwire from the Rust source, or null when absent. */
export function oscRouteTripwire() {
  const text = readRust('src/control/osc/server.rs');
  const match = text && /OSC_ROUTE_COUNT:\s*usize\s*=\s*(\d+)/.exec(text);
  return match ? Number(match[1]) : null;
}
