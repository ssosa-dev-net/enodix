; netterm's additions to the NSIS installer Tauri generates (bundle >
; windows > nsis > installerHooks). scripts/release.mjs gives Tauri a file
; that defines these, then includes this one:
;
;   NETTERM_PRODUCT      the product name (also Tauri's PRODUCTNAME)
;   NETTERM_PUBLISHER    the publisher (bundle > publisher, Tauri's MANUFACTURER)
;   NETTERM_DESCRIPTION  the short description (bundle > shortDescription)
;   NETTERM_OLD_VENDOR   the vendor key's name before the publisher was set
;                        (the identifier's second part, Tauri's default)
;   NETTERM_CLI          the headless binary beside the app
;   NETTERM_WHEEL        this version's Python wheel, in python\
;
; Tauri includes this file before its own defines and variables, so what is
; here runs only inside its sections (the four hook macros) or uses none of
; them (the version details and the function called at the window's start).

!include LogicLib.nsh

; The installer's own version details (Tauri sets the name, version and
; copyright).
VIAddVersionKey "CompanyName" "${NETTERM_PUBLISHER}"
VIAddVersionKey "Comments" "${NETTERM_DESCRIPTION}"

; Copies installed before the publisher was set remember their folder under
; the old vendor key, which this installer does not read: an upgrade would
; go to the default folder, and "Uninstall before installing" would not find
; the old copy. Before the first page, take the folder over from there (not
; over a folder given with /D=). Not run in silent mode, which shows no
; window: a silent upgrade of a copy in another folder needs /D=.
!if "${NETTERM_OLD_VENDOR}" != "${NETTERM_PUBLISHER}"
  !define MUI_CUSTOMFUNCTION_GUIINIT NettermOldInstallLocation
  Function NettermOldInstallLocation
    ReadRegStr $0 HKCU "Software\${NETTERM_PUBLISHER}\${NETTERM_PRODUCT}" ""
    ReadRegStr $1 HKCU "Software\${NETTERM_OLD_VENDOR}\${NETTERM_PRODUCT}" ""
    ${If} $0 == ""
    ${AndIf} $1 != ""
      WriteRegStr HKCU "Software\${NETTERM_PUBLISHER}\${NETTERM_PRODUCT}" "" $1
      ${If} $INSTDIR == "$LOCALAPPDATA\${NETTERM_PRODUCT}"
        StrCpy $INSTDIR $1
      ${EndIf}
    ${EndIf}
  FunctionEnd
!endif

; Before anything is copied: a running headless binary of this folder (a
; script using it) is handled as Tauri handles the app: asked about, closed
; in silent and passive mode. Otherwise the old one would stay, beside a new
; app.
!macro NSIS_HOOK_PREINSTALL
  !insertmacro CheckIfAppIsRunning "$INSTDIR\${NETTERM_CLI}" "${NETTERM_CLI}"
!macroend

; After the files are in place: the wheels of other versions go (their
; names change with the version, so Tauri's uninstaller would never remove
; them); the old vendor key goes, the folder being remembered under the
; new one now.
!macro NSIS_HOOK_POSTINSTALL
  FindFirst $R8 $R9 "$INSTDIR\python\${NETTERM_PRODUCT}-*-py3-none-any.whl"
  ${DoWhile} $R9 != ""
    ${If} $R9 != "${NETTERM_WHEEL}"
      Delete "$INSTDIR\python\$R9"
    ${EndIf}
    FindNext $R8 $R9
  ${Loop}
  FindClose $R8
  WriteRegStr SHCTX "${UNINSTKEY}" "Comments" "${NETTERM_DESCRIPTION}"
  !if "${NETTERM_OLD_VENDOR}" != "${MANUFACTURER}"
    DeleteRegKey HKCU "Software\${NETTERM_OLD_VENDOR}\${PRODUCTNAME}"
    DeleteRegKey /ifempty HKCU "Software\${NETTERM_OLD_VENDOR}"
  !endif
!macroend

; Before anything is removed: the app and the headless binary are closed
; (Tauri checks the app again right after: then nothing is running). With
; "Delete the application data" ticked, and only then (never in silent mode
; or on an update, which show no such box), the saved logins are removed
; from Windows Credential Manager while the headless binary is still here.
!macro NSIS_HOOK_PREUNINSTALL
  !insertmacro CheckIfAppIsRunning "$INSTDIR\${MAINBINARYNAME}.exe" "${PRODUCTNAME}"
  !insertmacro CheckIfAppIsRunning "$INSTDIR\${NETTERM_CLI}" "${NETTERM_CLI}"
  ${If} $DeleteAppDataCheckboxState = 1
  ${AndIf} $UpdateMode <> 1
    DetailPrint "$(nettermClearKeychain)"
    nsExec::ExecToLog '"$INSTDIR\${NETTERM_CLI}" clear-keychain'
    Pop $0
    ${If} $0 != "0"
      DetailPrint "$(nettermKeychainNotCleared)"
      MessageBox MB_ICONEXCLAMATION|MB_OK "$(nettermKeychainNotCleared)"
    ${EndIf}
  ${EndIf}
!macroend

; After Tauri's files are removed: every wheel of the product left in
; python\ (only those: other files there are not touched), then the folder,
; then the install folder if it is empty.
!macro NSIS_HOOK_POSTUNINSTALL
  FindFirst $R8 $R9 "$INSTDIR\python\${NETTERM_PRODUCT}-*-py3-none-any.whl"
  ${DoWhile} $R9 != ""
    Delete "$INSTDIR\python\$R9"
    FindNext $R8 $R9
  ${Loop}
  FindClose $R8
  RMDir "$INSTDIR\python"
  RMDir "$INSTDIR"
!macroend
