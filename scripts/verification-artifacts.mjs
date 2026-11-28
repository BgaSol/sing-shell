import assert from 'node:assert/strict';
import fs from 'node:fs';
import {execFileSync} from 'node:child_process';
import {targets} from './targets.mjs';

export function latestVerificationArtifacts(artifacts) {
  return targets.map(target => {
    const matches = artifacts.filter(a => a.name === `verification-${target}`)
      .sort((a, b) => Date.parse(b.created_at) - Date.parse(a.created_at) || b.id - a.id);
    const latest = matches[0];
    assert.ok(latest && !latest.expired, `missing current verification report: ${target}`);
    return latest.id;
  });
}

if (process.argv[1]?.replaceAll('\\', '/').endsWith('/verification-artifacts.mjs')) {
  const run = process.env.GITHUB_RUN_ID;
  assert.match(run ?? '', /^\d+$/);
  const pages = JSON.parse(execFileSync('gh', [
    'api', '--paginate', '--slurp',
    `repos/BgaSol/sing-shell/actions/runs/${run}/artifacts?per_page=100`,
  ], {encoding:'utf8'}));
  const ids = latestVerificationArtifacts(pages.flatMap(page => page.artifacts));
  // release.mjs still rejects unsuccessful reports and validates source and original bytes.
  fs.appendFileSync(process.env.GITHUB_OUTPUT, `ids=${ids.join(',')}\n`);
}
