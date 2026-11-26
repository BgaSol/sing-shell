import {test} from 'node:test';
import assert from 'node:assert/strict';
import {spawnSync} from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import {fileURLToPath} from 'node:url';

const compiler = path.join(process.env.LOCALAPPDATA ?? '', 'tauri/NSIS/makensis.exe');
test('NSIS preserves legacy update scope and /D, and rejects an implicit scope switch', {
  skip: process.platform !== 'win32' ? 'Windows only' : !fs.existsSync(compiler) && 'Tauri NSIS compiler is not installed',
}, () => {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'sing-shell-installer-'));
  const source = path.join(directory, 'test.nsi'), executable = path.join(directory, 'test.exe');
  const result = path.join(directory, 'result.txt'), installed = path.join(directory, '中文 existing install');
  const hooks = fileURLToPath(new URL('../src-tauri/installer-hooks.nsh', import.meta.url));
  const key = `Software\\Sing Shell-Test\\Installer-${process.pid}`;
  fs.mkdirSync(installed);
  fs.writeFileSync(path.join(installed, 'settings.txt'), 'keep');
  fs.writeFileSync(source, `
Unicode true
RequestExecutionLevel user
SilentInstall silent
OutFile "${executable}"
!include LogicLib.nsh
!include FileFunc.nsh
!include StrFunc.nsh
\${StrLoc}
!include "${hooks}"
!define MANUPRODUCTKEY "${key}"
!define UNINSTKEY "${key}\\Uninstall"
!define MAINBINARYNAME "test"
Var UpdateMode
Var MultiUser.InstallMode
Var MultiUser.Privileges
Function MultiUser.InstallMode.CurrentUser
  StrCpy $MultiUser.InstallMode "CurrentUser"
  ReadRegStr $INSTDIR HKCU "${key}" ""
FunctionEnd
Function un.MultiUser.InstallMode.CurrentUser
  StrCpy $MultiUser.InstallMode "CurrentUser"
  SetShellVarContext current
FunctionEnd
Function un.MultiUser.InstallMode.AllUsers
  StrCpy $MultiUser.InstallMode "AllUsers"
  SetShellVarContext all
FunctionEnd
Function .onInit
  StrCpy $MultiUser.InstallMode "AllUsers"
  StrCpy $INSTDIR "${directory}\\wrong-default"
  \${GetOptions} $CMDLINE "/UPDATE" $R0
  \${IfNot} \${Errors}
    StrCpy $UpdateMode 1
  \${EndIf}
  \${GetOptions} $CMDLINE "/Existing" $R0
  \${IfNot} \${Errors}
    WriteRegStr HKCU "${key}" "" "${installed}"
    WriteRegStr HKCU "${key}\\Uninstall" "InstallLocation" "${installed}"
  \${EndIf}
  \${GetOptions} $CMDLINE "/Stale" $R0
  \${IfNot} \${Errors}
    WriteRegStr HKCU "${key}" "" "${installed}"
  \${EndIf}
  \${GetOptions} $CMDLINE "/CurrentUser" $R0
  \${IfNot} \${Errors}
    Call MultiUser.InstallMode.CurrentUser
  \${EndIf}
FunctionEnd
Section
  !insertmacro NSIS_HOOK_PREINSTALL
  FileOpen $0 "${result}" w
  FileWriteUTF16LE $0 "$MultiUser.InstallMode|$INSTDIR"
  FileClose $0
  WriteUninstaller "${installed}\\uninstall.exe"
SectionEnd
Function un.onInit
  StrCpy $MultiUser.InstallMode "AllUsers"
FunctionEnd
Section Uninstall
  !insertmacro NSIS_HOOK_PREUNINSTALL
  FileOpen $0 "${result}" w
  FileWriteUTF16LE $0 "$MultiUser.InstallMode|$INSTDIR"
  FileClose $0
SectionEnd
`);
  function run(args, expected, output) {
    spawnSync('reg.exe', ['delete', `HKCU\\${key}`, '/f'], {windowsHide:true, stdio:'ignore'});
    fs.rmSync(result, {force:true});
    const run = spawnSync(executable, ['/S', ...args], {encoding:'utf8', windowsHide:true, timeout:15_000});
    assert.equal(run.status, expected, run.stdout + run.stderr);
    if (output) assert.equal(fs.readFileSync(result, 'utf16le'), output);
    else assert.equal(fs.existsSync(result), false);
    assert.equal(fs.readFileSync(path.join(installed, 'settings.txt'), 'utf8'), 'keep');
  }
  try {
    const build = spawnSync(compiler, ['/V2', '/INPUTCHARSET', 'UTF8', source], {encoding:'utf8', windowsHide:true});
    assert.equal(build.status, 0, build.stdout + build.stderr);
    run(['/Existing', '/UPDATE'], 0, `CurrentUser|${installed}`);
    fs.rmSync(result);
    const uninstall = spawnSync(path.join(installed, 'uninstall.exe'), ['/S', `_?=${installed}`], {windowsHide:true, windowsVerbatimArguments:true, timeout:15_000});
    assert.equal(uninstall.status, 0);
    assert.equal(fs.readFileSync(result, 'utf16le'), `CurrentUser|${installed}`);
    const custom = path.join(directory, '中文 custom install');
    run([`/D=${custom}`], 0, `AllUsers|${custom}`);
    run(['/ARGS', '/D=ignored', `/D=${custom}`], 0, `AllUsers|${custom}`);
    run(['/Stale', `/D=${custom}`], 0, `AllUsers|${custom}`);
    run(['/Existing', '/CurrentUser', `/D=${installed}`], 0, `CurrentUser|${installed}`);
    run(['/Existing', '/AllUsers'], 2);
  } finally {
    spawnSync('reg.exe', ['delete', `HKCU\\${key}`, '/f'], {windowsHide:true, stdio:'ignore'});
    assert.equal(path.dirname(directory), os.tmpdir());
    assert.ok(path.basename(directory).startsWith('sing-shell-installer-'));
    fs.rmSync(directory, {recursive:true, force:true});
  }
});
