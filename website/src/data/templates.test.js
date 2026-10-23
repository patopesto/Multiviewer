import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

import { repoPath } from './rust-source.js';
import { templateDownloads } from './templates.js';

test('template downloads point at committed files', () => {
  assert.ok(templateDownloads.length > 0, 'no template downloads');
  for (const item of templateDownloads) {
    assert.ok(item.title.length > 0, 'missing title');
    assert.ok(item.href.startsWith('/downloads/'), `bad href: ${item.href}`);
    const file = repoPath(`website/public${item.href}`);
    assert.ok(file, `missing file for ${item.href}`);
    if (item.href.endsWith('.companionconfig')) {
      const data = JSON.parse(readFileSync(file, 'utf8'));
      assert.equal(data.type, 'page');
      assert.ok(data.page?.name?.length > 0, 'missing page name');
    }
  }
});
