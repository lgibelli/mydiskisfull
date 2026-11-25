; MyDiskIsFull's additions to Tauri's installer (tauri.conf.json: bundle >
; windows > nsis > installerHooks).
;
; The watcher, mdif-watch.exe, runs from the install folder, and Windows lets
; no one replace or delete a running program's file: it's stopped before an
; update replaces it and started again after, and stopped for good before an
; uninstall. mdif does each (`mdif agent stop`, `start`, `uninstall`), the same
; as for anyone at a command line; nsExec runs it without a console window.

!macro NSIS_HOOK_PREINSTALL
  ; An update (or the same version again): mdif is already here.
  ${If} ${FileExists} "$INSTDIR\mdif.exe"
    nsExec::Exec '"$INSTDIR\mdif.exe" agent stop'
    Pop $0
  ${EndIf}
!macroend

!macro NSIS_HOOK_POSTINSTALL
  ; Started again as sign-in would start it: not if it isn't set up, nor if
  ; it's switched off in Task Manager's Startup apps.
  nsExec::Exec '"$INSTDIR\mdif.exe" agent start'
  Pop $0
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ${If} $UpdateMode = 1
    ; Replaced by another version: it stays set up.
    nsExec::Exec '"$INSTDIR\mdif.exe" agent stop'
    Pop $0
  ${Else}
    ; It no longer starts at sign-in. Tauri's uninstaller removes the app's
    ; own Run value; Task Manager's switch for it goes here.
    nsExec::Exec '"$INSTDIR\mdif.exe" agent uninstall'
    Pop $0
    DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run" "${PRODUCTNAME}"
  ${EndIf}
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ; "Delete the application data" includes mdif's: the history, its log and
  ; the measurements kept, in %LOCALAPPDATA%\MyDiskIsFull (watch_cmd.rs's
  ; data_dir). Unticked, they stay for a later install.
  ${If} $DeleteAppDataCheckboxState = 1
  ${AndIf} $UpdateMode <> 1
    SetShellVarContext current
    ${If} "$LOCALAPPDATA" != ""
      RMDir /r "$LOCALAPPDATA\MyDiskIsFull"
    ${EndIf}
  ${EndIf}
!macroend
