import {execFileSync} from 'node:child_process';
import assert from 'node:assert/strict';
import {targets} from './targets.mjs';
const id=process.env.PACKAGE_RUN_ID;assert.match(id??'',/^\d+$/);
const api=endpoint=>JSON.parse(execFileSync('gh',['api',`repos/BgaSol/sing-shell/${endpoint}`],{encoding:'utf8'}));
const run=api(`actions/runs/${id}`);assert.equal(run.status,'completed');assert.equal(run.path,'.github/workflows/package.yml');
const artifacts=api(`actions/runs/${id}/artifacts`).artifacts;
for(const target of targets){const matches=artifacts.filter(a=>a.name===`package-${target}`&&!a.expired);assert.equal(matches.length,1,`missing original signed packages: ${target}`);}
