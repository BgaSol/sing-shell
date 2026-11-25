import { test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../', import.meta.url));

test('local documentation links resolve', () => {
  const documents = ['README.md', 'AGENTS.md', ...fs.readdirSync(path.join(root, 'docs'), { recursive: true }).filter(name => name.endsWith('.md')).map(name => path.join('docs', name))];
  for (const document of documents) {
    const text = fs.readFileSync(path.join(root, document), 'utf8');
    for (const match of text.matchAll(/\[[^\]]+\]\(([^)]+)\)/g)) {
      const target = match[1].split('#')[0];
      if (!target || /^[a-z]+:/i.test(target)) continue;
      assert.ok(fs.existsSync(path.resolve(root, path.dirname(document), decodeURIComponent(target))), `${document}: missing ${target}`);
    }
  }
});

test('every npm command called by a workflow exists', () => {
  const pkg = JSON.parse(fs.readFileSync(path.join(root, 'package.json')));
  for (const workflow of fs.readdirSync(path.join(root, '.github/workflows'))) {
    const text = fs.readFileSync(path.join(root, '.github/workflows', workflow), 'utf8');
    for (const match of text.matchAll(/npm run ([\w:-]+)/g)) {
      assert.ok(pkg.scripts[match[1]], `${workflow}: nonexistent npm command ${match[1]}`);
    }
  }
});
