; highestAvailable does not elevate a standard account during a system-level update.
!define MULTIUSER_INIT_FUNCTIONQUIT SingShellAuthorizeInstaller
!define MULTIUSER_INIT_un.FUNCTIONQUIT un.SingShellAuthorizeInstaller
!define MULTIUSER_INIT_TEXT_ALLUSERSNOTPOSSIBLE "System installation requires administrator approval. Click OK to request it."

!macro SingShellReadInstallDirectory
  System::Call 'kernel32::GetCommandLineW()w .r0'
  StrCpy $R2 ""
  ${Do}
    ${StrLoc} $R1 $0 "/D=" ">"
    ${IfThen} $R1 == "" ${|} ${ExitDo} ${|}
    IntOp $R1 $R1 + 3
    StrCpy $0 $0 "" $R1
    StrCpy $R2 $0
  ${Loop}
  ${If} $R2 != ""
    StrCpy $R1 $R2 1 -1
    ${If} $R1 == '$\"'
      StrCpy $R2 $R2 -1
    ${EndIf}
    StrCpy $R1 $R2 1
    ${If} $R1 == '$\"'
      StrCpy $R2 $R2 "" 1
    ${EndIf}
  ${EndIf}
!macroend

Function SingShellAuthorizeInstaller
  ${GetParameters} $R0
  !insertmacro SingShellReadInstallDirectory
  ${If} $R2 != ""
    StrCpy $R0 "$R0 /D=$R2"
  ${EndIf}
  ClearErrors
  ExecShell "runas" "$EXEPATH" "$R0" SW_SHOWNORMAL
  ${If} ${Errors}
    SetErrorLevel 1223
  ${EndIf}
  Quit
FunctionEnd

Function un.SingShellAuthorizeInstaller
  ${un.GetParameters} $R0
  ClearErrors
  ExecShell "runas" "$INSTDIR\uninstall.exe" "$R0 /AllUsers" SW_SHOWNORMAL
  ${If} ${Errors}
    SetErrorLevel 1223
  ${EndIf}
  Quit
FunctionEnd

!macro NSIS_HOOK_PREINSTALL
  Push $0
  Push $R0
  Push $R1
  Push $R2
  ReadRegStr $R0 HKCU "${UNINSTKEY}" "InstallLocation"
  ReadRegStr $R1 HKLM "${UNINSTKEY}" "InstallLocation"

  ; Older currentUser installers have no MultiUser marker or explicit update scope.
  ${If} $UpdateMode = 1
    ${GetOptions} $CMDLINE "/AllUsers" $R2
    ${If} ${Errors}
      ${GetOptions} $CMDLINE "/CurrentUser" $R2
      ${If} ${Errors}
      ${AndIf} $R0 != ""
      ${AndIf} $R1 == ""
        Call MultiUser.InstallMode.CurrentUser
      ${EndIf}
    ${EndIf}
  ${EndIf}

  ${If} $MultiUser.InstallMode == "AllUsers"
    StrCpy $R2 $R0
  ${Else}
    StrCpy $R2 $R1
  ${EndIf}
  ${If} $R2 != ""
    MessageBox MB_OK|MB_ICONSTOP "Sing Shell is installed for a different scope. Exit Sing Shell, uninstall that installation without deleting app data, then run this installer again." /SD IDOK
    SetErrorLevel 2
    Abort
  ${EndIf}

  ; MultiUser initialization overwrites NSIS's /D override; restore the final argument.
  !insertmacro SingShellReadInstallDirectory
  ${If} $R2 != ""
    StrCpy $INSTDIR $R2
  ${EndIf}
  SetOutPath $INSTDIR
  Pop $R2
  Pop $R1
  Pop $R0
  Pop $0
!macroend

; Keep user-disabled entries and their paths untouched during an in-place update.
!macro NSIS_HOOK_PREUNINSTALL
  Push $R0
  ; The uninstaller's own location wins over another user's MultiUser preference.
  ReadRegStr $R0 HKLM "${MANUPRODUCTKEY}" ""
  ${If} $R0 == $INSTDIR
    ${If} $MultiUser.Privileges != "Admin"
    ${AndIf} $MultiUser.Privileges != "Power"
      Call un.SingShellAuthorizeInstaller
    ${EndIf}
    Call un.MultiUser.InstallMode.AllUsers
  ${Else}
    ReadRegStr $R0 HKCU "${MANUPRODUCTKEY}" ""
    ${If} $R0 == $INSTDIR
      Call un.MultiUser.InstallMode.CurrentUser
    ${Else}
      MessageBox MB_OK|MB_ICONSTOP "Sing Shell's installation location does not match its registration. Reinstall it before uninstalling." /SD IDOK
      SetErrorLevel 2
      Abort
    ${EndIf}
  ${EndIf}
  ${If} $UpdateMode <> 1
    ReadRegStr $R0 HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "com.bgasol.singshell"
    ${If} $R0 == '"$INSTDIR\${MAINBINARYNAME}.exe" --login-start'
      DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "com.bgasol.singshell"
    ${EndIf}
  ${EndIf}
  Pop $R0
!macroend
