// Settings tables (protocols and the canvas), discovered from `// @docs`
// directives in the Rust config structs. Runs at Astro build time (SSG) so the
// docs never drift from the code.
//
// Directive grammar, on the item it applies to:
//   struct: // @docs protocol="NDI" direction=input [section="macOS"]
//   field:  // @docs [label="…"] [options="a,b"] [group="f,g"] [desc="…"]
//           [hidden] [default="…"]
import { readFileSync } from 'node:fs';
import path from 'node:path';

import { repoPath, repoRoot, walkRustFiles } from './rust-source.js';

const STRUCT_KEYS = new Set(['protocol', 'direction', 'section']);
const FIELD_KEYS = new Set(['label', 'options', 'group', 'desc', 'hidden', 'default']);
const SECTION_RANK = { macos: 0, windows: 1, linux: 2 };

function sourcesDir() {
  return repoPath('src/sources');
}

function rustSourceFiles() {
  const dir = sourcesDir();
  const files = dir ? walkRustFiles(dir) : [];
  const cfg = repoPath('src/config.rs');
  if (cfg) files.push(cfg);
  return files;
}

function firstParagraph(lines) {
  const out = [];
  for (const line of lines) {
    if (line.trim() === '') {
      if (out.length) break;
      continue;
    }
    out.push(line.trim());
  }
  return out.join(' ');
}

function parseDirective(text) {
  const out = {};
  const re = /(\w+)\s*=\s*"([^"]*)"|(\w+)\s*=\s*(\S+)|(\w+)/g;
  for (const match of text.matchAll(re)) {
    if (match[1] !== undefined) out[match[1]] = match[2];
    else if (match[3] !== undefined) out[match[3]] = match[4];
    else out[match[5]] = true;
  }
  return out;
}

function parsePrefix(block, baseLine) {
  const directives = {};
  const consumed = [];
  block.split('\n').forEach((raw, index) => {
    const line = raw.trim();
    if (line.startsWith('// @docs')) {
      Object.assign(directives, parseDirective(line.slice(8)));
      consumed.push(baseLine + index);
    }
  });
  return { directives, consumed };
}

const ITEM_RE =
  /(?:^|\n)((?:(?:[ \t]*\/\/[^\n]*|[ \t]*#[^\n]*|[ \t]*)\n)*)[ \t]*pub (struct|enum) (\w+)[^\n]*?\{([\s\S]*?)\n?\}/gd;

function parseFields(body, baseLine) {
  const fields = [];
  const consumed = [];
  let doc = [];
  let directives = {};
  let pending = [];
  body.split('\n').forEach((raw, index) => {
    const line = raw.trim();
    if (line.startsWith('///')) {
      doc.push(line.slice(3));
      return;
    }
    if (line.startsWith('// @docs')) {
      Object.assign(directives, parseDirective(line.slice(8)));
      pending.push(baseLine + index);
      return;
    }
    if (line === '' || line.startsWith('#[') || line.startsWith('//')) return;
    const code = line.replace(/\/\/.*$/, '').trim().replace(/,$/, '').trim();
    const match = /^pub (\w+)\s*:\s*(.+)$/.exec(code);
    if (match) {
      fields.push({ name: match[1], type: match[2].trim(), doc: firstParagraph(doc), directives });
      consumed.push(...pending);
    }
    doc = [];
    directives = {};
    pending = [];
  });
  return { fields, consumed };
}

function parseVariants(body) {
  const variants = [];
  let isDefault = false;
  for (const raw of body.split('\n')) {
    const line = raw.trim();
    if (line.startsWith('#[default]')) {
      isDefault = true;
      continue;
    }
    if (line.startsWith('///') || line.startsWith('//')) continue;
    const match = /^([A-Z]\w*)\s*[({,=]/.exec(line);
    if (match) {
      variants.push({ name: match[1], default: isDefault });
      isDefault = false;
    }
  }
  return variants;
}

function parseLabels(text, name) {
  const labels = new Map();
  const optionLabels = [];
  const impl = new RegExp(`impl ${name}\\b[^{]*\\{([\\s\\S]*?)\\n\\}`, 'g').exec(text);
  const fn = impl && /fn label\([^)]*\)[^{]*\{([\s\S]*?)\n[ \t]{4}\}/.exec(impl[1]);
  if (!fn) return { labels, optionLabels };
  const arm = /(?:Self|(\w+))::(\w+)\s*(?:\([^)]*\)|\{[^}]*\})?\s*=>\s*"([^"]*)"/g;
  for (const [, , variant, label] of fn[1].matchAll(arm)) {
    labels.set(variant, [...(labels.get(variant) ?? []), label]);
    optionLabels.push(label);
  }
  return { labels, optionLabels };
}

function parseDefaultImpls(text) {
  const impls = new Map();
  for (const [, name, body] of text.matchAll(/impl Default for (\w+)\s*\{([\s\S]*?)\n\}/g)) {
    const fields = new Map();
    for (const [, field, expr] of body.matchAll(/^[ \t]*(\w+)\s*:\s*(.+?),?[ \t]*$/gm)) {
      fields.set(field, expr.trim());
    }
    impls.set(name, fields);
  }
  return impls;
}

function lineStarts(text) {
  const starts = [0];
  for (let i = 0; i < text.length; i += 1) if (text[i] === '\n') starts.push(i + 1);
  return starts;
}

function lineOf(starts, offset) {
  let lo = 0;
  let hi = starts.length - 1;
  while (lo < hi) {
    const mid = (lo + hi + 1) >> 1;
    if (starts[mid] <= offset) lo = mid;
    else hi = mid - 1;
  }
  return lo;
}

function parseRustIndex() {
  const structs = new Map();
  const enums = new Map();
  const misplaced = [];
  for (const file of rustSourceFiles()) {
    const text = readFileSync(file, 'utf8');
    const starts = lineStarts(text);
    const consumed = new Set();
    const defaults = parseDefaultImpls(text);
    for (const match of text.matchAll(ITEM_RE)) {
      const prefix = parsePrefix(match[1], lineOf(starts, match.indices[1][0]));
      prefix.consumed.forEach((line) => consumed.add(line));
      const [, , kind, name, body] = match;
      if (kind === 'struct') {
        const parsed = parseFields(body, lineOf(starts, match.indices[4][0]));
        parsed.consumed.forEach((line) => consumed.add(line));
        structs.set(name, {
          fields: parsed.fields,
          defaults: defaults.get(name) ?? new Map(),
          directives: prefix.directives,
        });
      } else {
        enums.set(name, { variants: parseVariants(body), ...parseLabels(text, name) });
      }
    }
    const rel = path.relative(repoRoot(), file);
    text.split('\n').forEach((line, index) => {
      if (line.trim().startsWith('// @docs') && !consumed.has(index)) {
        misplaced.push(`${rel}:${index + 1}`);
      }
    });
  }
  return { structs, enums, misplaced };
}

const INTEGER_RE = /^(u|i)\d+$/;
const FLOAT_RE = /^f\d+$/;

function baseType(type) {
  return type
    .replace(/^Option<(.+)>$/, '$1')
    .trim()
    .split(/[<>\s]/)[0]
    .split('::')
    .pop();
}

function prettyType(base) {
  if (INTEGER_RE.test(base) || base === 'usize' || base === 'isize') return 'integer';
  if (FLOAT_RE.test(base)) return 'number';
  if (base === 'bool') return 'toggle';
  if (base === 'String') return 'text';
  if (/^\[/.test(base)) return 'color';
  return 'choice';
}

function prettify(name) {
  return name
    .split('_')
    .map((word, index) => (index === 0 ? word.charAt(0).toUpperCase() + word.slice(1) : word))
    .join(' ');
}

function scalarDefault(base) {
  if (INTEGER_RE.test(base) || base === 'usize' || base === 'isize') return '0';
  if (FLOAT_RE.test(base)) return '0';
  if (base === 'bool') return 'false';
  if (base === 'String') return '""';
  return '';
}

function renderDefault(expr, enumType) {
  let value = expr.trim().replace(/,$/, '');
  const some = /^Some\(([\s\S]*)\)$/.exec(value);
  if (some) value = some[1].trim();
  if (value === 'None') return 'Auto';
  if (value === 'true' || value === 'false') return value;
  if (/^-?\d+(\.\d+)?$/.test(value)) return value;
  if (/^\[.*\]$/.test(value)) return value;
  if (/^(\w+::)?new\(\)$/.test(value)) return '""';
  if (/^(\w+::)?default\(\)$/.test(value)) return enumDefaultLabel(enumType);
  const quoted = /"([^"]*)"/.exec(value);
  if (quoted) return `"${quoted[1]}"`;
  const variant = /^(\w+)::(\w+)\s*(?:\(|\{|$)/.exec(value);
  if (variant) {
    const labels = enumType?.labels.get(variant[2]);
    return labels?.length === 1 ? labels[0] : '';
  }
  return '';
}

function enumDefaultLabel(enumType) {
  const def = enumType?.variants.find((variant) => variant.default);
  const labels = def && enumType.labels.get(def.name);
  return labels?.length === 1 ? labels[0] : '';
}

function splitList(value) {
  return value ? value.split(',').map((item) => item.trim()).filter(Boolean) : [];
}

function fieldInfo(index, structName, fieldName) {
  const struct = index.structs.get(structName);
  const field = struct?.fields.find((candidate) => candidate.name === fieldName);
  if (!field) return { options: [], type: 'text', default: '' };
  const base = baseType(field.type);
  const enumType = index.enums.get(base);
  const options = enumType?.optionLabels ?? [];
  const explicit = struct.defaults.get(field.name);
  let fallback = '';
  if (explicit !== undefined) fallback = renderDefault(explicit, enumType);
  else if (/^Option</.test(field.type)) fallback = 'Auto';
  else if (enumType) fallback = enumDefaultLabel(enumType);
  else fallback = scalarDefault(base);
  return { options, type: enumType ? 'select' : prettyType(base), default: fallback };
}

/** Rows for one struct, with nested structs and groups resolved. */
function collect(index, structName, prefix, visited) {
  const struct = index.structs.get(structName);
  const rows = [];
  const accounted = new Set();
  const consumed = new Set();
  for (const field of struct.fields) {
    if (consumed.has(field.name)) continue;
    const directive = field.directives ?? {};
    const path = `${prefix}${field.name}`;
    if (directive.hidden) {
      accounted.add(path);
      continue;
    }
    const nested = index.structs.get(baseType(field.type));
    const expands = !directive.group && !directive.label && !directive.options;
    if (expands && nested?.fields.length && !visited.has(baseType(field.type))) {
      const sub = collect(index, baseType(field.type), `${path}.`, new Set([...visited, baseType(field.type)]));
      rows.push(...sub.rows);
      for (const accountedPath of sub.accounted) accounted.add(accountedPath);
      continue;
    }
    if (directive.group) {
      for (const member of [field.name, ...splitList(directive.group)]) {
        consumed.add(member);
        accounted.add(`${prefix}${member}`);
      }
      rows.push({
        label: directive.label ?? prettify(field.name),
        type: 'select',
        options: splitList(directive.options),
        default: directive.default ?? '',
        doc: directive.desc ?? field.doc,
      });
      continue;
    }
    accounted.add(path);
    const info = fieldInfo(index, structName, field.name);
    const options = splitList(directive.options).length ? splitList(directive.options) : info.options;
    rows.push({
      label: directive.label ?? prettify(field.name),
      type: options.length ? 'select' : info.type,
      options,
      default: directive.default ?? info.default,
      doc: directive.desc ?? field.doc,
    });
  }
  return { rows, accounted };
}

function allPaths(index, structName, prefix, visited) {
  const struct = index.structs.get(structName);
  const paths = [];
  for (const field of struct.fields) {
    const base = baseType(field.type);
    const nested = index.structs.get(base);
    const path = `${prefix}${field.name}`;
    if (nested?.fields.length && !visited.has(base)) {
      paths.push(...allPaths(index, base, `${path}.`, new Set([...visited, base])));
    } else {
      paths.push(path);
    }
  }
  return paths;
}

function sectionRank(label) {
  return SECTION_RANK[label?.toLowerCase()] ?? 3;
}

/**
 * @typedef {object} Setting
 * @property {string} label
 * @property {string} type
 * @property {string[]} options
 * @property {string} default
 * @property {string} doc
 *
 * @typedef {object} SettingsGroup
 * @property {string} [label]
 * @property {Setting[]} settings
 *
 * @typedef {object} SettingsEntry
 * @property {string} name
 * @property {string} direction
 * @property {SettingsGroup[]} groups
 */

/**
 * Settings groups by name and direction (protocols and the canvas), discovered
 * from `// @docs`.
 * @returns {SettingsEntry[]}
 */
export function getSettings() {
  if (!sourcesDir()) return [];
  const index = parseRustIndex();
  const entries = new Map();
  for (const [structName, struct] of index.structs) {
    const { protocol, direction = 'input', section } = struct.directives ?? {};
    if (!protocol) continue;
    const key = `${protocol}\u0000${direction}`;
    if (!entries.has(key)) entries.set(key, { name: protocol, direction, groups: [] });
    entries.get(key).groups.push({
      label: section,
      rank: sectionRank(section),
      settings: collect(index, structName, '', new Set([structName])).rows,
    });
  }
  return [...entries.values()].map(({ name, direction, groups }) => ({
    name,
    direction,
    groups: groups
      .sort((a, b) => a.rank - b.rank)
      .map(({ label, settings }) => ({ label, settings })),
  }));
}

/** Config structs in `src/sources` without a `@docs protocol` directive.
 * @returns {string[]}
 */
export function undocumentedConfigStructs() {
  if (!sourcesDir()) return [];
  return [...parseRustIndex().structs]
    .filter(([name, struct]) => /(Source|Output)Config$|Settings$/.test(name) && !struct.directives.protocol)
    .map(([name]) => name)
    .sort();
}

/** Config fields neither shown as a row nor marked `@docs hidden`.
 * @returns {string[]}
 */
export function undocumentedConfigFields() {
  if (!sourcesDir()) return [];
  const index = parseRustIndex();
  const missing = [];
  for (const [structName, struct] of index.structs) {
    if (!struct.directives.protocol) continue;
    const { accounted } = collect(index, structName, '', new Set([structName]));
    for (const path of allPaths(index, structName, '', new Set([structName]))) {
      if (!accounted.has(path)) missing.push(`${structName}.${path}`);
    }
  }
  return [...new Set(missing)].sort();
}

/** `@docs` keys that are not recognized; empty when every directive is valid.
 * @returns {string[]}
 */
export function unknownDirectives() {
  if (!sourcesDir()) return [];
  const index = parseRustIndex();
  const bad = [];
  for (const [name, struct] of index.structs) {
    for (const key of Object.keys(struct.directives ?? {})) {
      if (!STRUCT_KEYS.has(key)) bad.push(`${name}: @docs ${key}`);
    }
    for (const field of struct.fields) {
      for (const key of Object.keys(field.directives ?? {})) {
        if (!FIELD_KEYS.has(key)) bad.push(`${name}.${field.name}: @docs ${key}`);
      }
    }
  }
  return bad.sort();
}

/** `@docs` lines that no struct/enum/field claims, as `path:line`; empty when all are attached.
 * @returns {string[]}
 */
export function misplacedDirectives() {
  if (!sourcesDir()) return [];
  return parseRustIndex().misplaced;
}
