import fs from 'node:fs';
import {matrix} from './targets.mjs';
fs.appendFileSync(process.env.GITHUB_OUTPUT,`matrix=${JSON.stringify(matrix)}\n`);
