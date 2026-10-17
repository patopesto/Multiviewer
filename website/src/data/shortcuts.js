// Keyboard shortcuts, read from `src/ui/shortcuts.rs` at Astro build time so the
// docs page matches the in-app reference and cannot drift.
import { readRust } from './rust-source.js';

/** Body of a `fn name(...) { ... }` by brace matching. */
function fnBody(text, name) {
  const match = new RegExp(`fn ${name}\\([^)]*\\)[^{]*\\{`).exec(text);
  if (!match) return null;
  let depth = 1;
  let i = match.index + match[0].length;
  const start = i;
  while (i < text.length && depth > 0) {
    if (text[i] === '{') depth += 1;
    else if (text[i] === '}') depth -= 1;
    i += 1;
  }
  return text.slice(start, i - 1);
}

/** Match arms, joining multi-line patterns that continue with `|`. */
function parseArms(body) {
  const arms = [];
  let pattern = '';
  for (const raw of body.split('\n')) {
    const line = raw.trim();
    if (line === '' || line.startsWith('//')) continue;
    const index = line.indexOf('=>');
    if (index === -1) {
      pattern = `${pattern}${pattern ? ' ' : ''}${line}`;
      continue;
    }
    arms.push({ pattern: `${pattern} ${line.slice(0, index)}`.trim(), value: line.slice(index + 2) });
    pattern = '';
  }
  return arms;
}

function parseValue(value) {
  const code = value.replace(/,\s*$/, '').trim();
  const formatted = /^format!\(([\s\S]*)\)$/.exec(code);
  if (formatted) {
    const literal = /"((?:[^"\\]|\\.)*)"/.exec(formatted[1]);
    return literal ? literal[1].replace(/\{MOD\}/g, 'Cmd/Ctrl') : '';
  }
  const literal = /^"((?:[^"\\]|\\.)*)"/.exec(code);
  return literal ? literal[1] : '';
}

/** variant name -> value, from a `match self` helper such as `fn label`. */
function variantMap(body) {
  const map = new Map();
  if (!body) return map;
  for (const { pattern, value } of parseArms(body)) {
    for (const [, variant] of pattern.matchAll(/(?:Shortcut|Self)::(\w+)/g)) {
      map.set(variant, parseValue(value));
    }
  }
  return map;
}

function globalOrder(body) {
  return body ? [...body.matchAll(/Shortcut::(\w+)/g)].map(([, variant]) => variant) : [];
}

/**
 * @typedef {object} Shortcut
 * @property {string} label
 * @property {string} key
 * @property {string} detail
 *
 * @typedef {object} ShortcutGroup
 * @property {string} category
 * @property {Shortcut[]} shortcuts
 */

/**
 * Shortcuts grouped by category, in `all_global()` order.
 * @returns {ShortcutGroup[]}
 */
export function getShortcuts() {
  const text = readRust('src/ui/shortcuts.rs');
  if (!text) return [];
  const labels = variantMap(fnBody(text, 'label'));
  const keys = variantMap(fnBody(text, 'key'));
  const details = variantMap(fnBody(text, 'detail'));
  const categories = variantMap(fnBody(text, 'category'));

  const groups = [];
  for (const variant of globalOrder(fnBody(text, 'all_global'))) {
    const category = categories.get(variant) ?? 'Other';
    let group = groups.find((candidate) => candidate.category === category);
    if (!group) {
      group = { category, shortcuts: [] };
      groups.push(group);
    }
    group.shortcuts.push({
      label: labels.get(variant) ?? variant,
      key: keys.get(variant) ?? '',
      detail: (details.get(variant) ?? '').trim(),
    });
  }
  return groups;
}
