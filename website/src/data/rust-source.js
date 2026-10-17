// Shared plumbing for the build-time modules that read settings and shortcuts
// out of the Rust source: locating repo files, reading them, and walking trees.
import { existsSync, readFileSync, readdirSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import path from 'node:path';

/** Resolve a repo-relative path, whether the site is built from `website/` or the repo root. */
export function repoPath(relative) {
  const here = path.dirname(fileURLToPath(import.meta.url));
  const candidates = [
    path.resolve(here, '../../..', relative),
    path.resolve(process.cwd(), '..', relative),
    path.resolve(process.cwd(), relative),
  ];
  return candidates.find((candidate) => existsSync(candidate)) ?? null;
}

/** Read a repo-relative Rust file, or `null` when it is absent. */
export function readRust(relative) {
  const file = repoPath(relative);
  return file ? readFileSync(file, 'utf8') : null;
}

/** Every `.rs` file under an absolute directory. */
export function walkRustFiles(dir) {
  const files = [];
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) files.push(...walkRustFiles(full));
    else if (entry.name.endsWith('.rs')) files.push(full);
  }
  return files;
}

/** Repo root, for shortening paths in error messages. */
export function repoRoot() {
  return path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../..');
}
