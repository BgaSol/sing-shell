// Regenerate the checked-in tray drawings with the existing Tauri icon tool.
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import {execFileSync} from 'node:child_process';
const temporary=fs.mkdtempSync(path.join(os.tmpdir(),'sing-shell-tray-'));
try {
  for(const template of [false,true]) for(const state of ['idle','busy','running','attention']) {
    const name=`tray-${template?'template-':''}${state}`;
    const ink=template?'#000':'#20211f';
    const shell=`<path d="M11 24C8 20 3 14 4 10C4 7 7 6 10 8C10 4 14 3 16 6C19 3 23 4 23 8C26 6 29 8 28 11C28 15 24 21 21 24Z" fill="${template?'none':'#ffde75'}"/><path d="M10 9L14 24M16 7V24M23 9L19 24" fill="none"/><rect x="10" y="24" width="12" height="4" rx="2" fill="${template?'none':'#42c69b'}"/>`;
    const badge=state==='idle'?'':`<circle cx="25" cy="25" r="6" fill="${template?'#000':state==='attention'?'#ffb7cf':state==='running'?'#42c69b':'#ffde75'}" stroke="${template?'#000':ink}" stroke-width="1.5"/>`+(state==='running'?'<path d="M22 25L24 27L28 23"/>':state==='attention'?'<path d="M25 22V25M25 27V27.1"/>':'<path d="M23 23V27M27 23V27"/>');
    const source=`<svg xmlns="http://www.w3.org/2000/svg" width="256" height="256" viewBox="0 0 32 32"><defs><mask id="body"><rect width="32" height="32" fill="white"/>${state==='idle'?'':'<circle cx="25" cy="25" r="7" fill="black"/>'}</mask>${template?'<mask id="badge"><rect width="32" height="32" fill="white"/>'+ (state==='running'?'<path d="M22 25L24 27L28 23"/>':state==='attention'?'<path d="M25 22V25M25 27V27.1"/>':'<path d="M23 23V27M27 23V27"/>') .replaceAll('<path','<path stroke="black" stroke-width="1.8" fill="none" stroke-linecap="round" stroke-linejoin="round"')+'</mask>':''}</defs><g stroke="${ink}" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" mask="url(#body)">${shell}</g><g fill="none" stroke="${template?'#000':ink}" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"${template?' mask="url(#badge)"':''}>${template?badge.replace(/<path.*$/,''):badge}</g></svg>\n`;
    const svg=path.resolve(`assets/${name}.svg`);fs.writeFileSync(svg,source);
    const output=path.join(temporary,name);
    execFileSync(process.execPath,['node_modules/@tauri-apps/cli/tauri.js','icon',svg,'--output',output],{stdio:'pipe'});
    fs.copyFileSync(path.join(output,'32x32.png'),`src-tauri/icons/${name}.png`);
    fs.copyFileSync(path.join(output,'64x64.png'),`src-tauri/icons/${name}@2x.png`);
  }
} finally { fs.rmSync(temporary,{recursive:true,force:true}); }
