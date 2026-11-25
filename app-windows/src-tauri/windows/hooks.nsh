; MyDiskIsFull's additions to Tauri's installer (tauri.conf.json: bundle >
; windows > nsis > installerHooks).
;
; The watcher, mdif-watch.exe, runs from the install folder, and Windows lets
; no one replace or delete a running program's file: it's stopped before an
; update replaces it and started again after, and stopped for good before an
; uninstall. mdif does each (`mdif agent stop`, `start`, `uninstall`), the same
; as for anyone at a command line; nsExec runs it without a console window.

; Whether <file> is in use: a running program's file can't be opened to
; write (nothing is written, and it's left as it is). 1 or 0 in <var>.
!macro MdifInUse file var
  ${If} ${FileExists} "${file}"
    ClearErrors
    FileOpen $2 "${file}" a
    ${If} ${Errors}
      StrCpy ${var} 1
    ${Else}
      FileClose $2
    ${EndIf}
  ${EndIf}
!macroend

; The install stops, nothing changed: the installer's stop called off (one
; of the user's stays), so the watcher goes on once its sample ends. $4: a
; stop was there before this install. An mdif from before --after-install
; (2): started plainly, unless that stop was there.
!macro MdifGiveUp message
  nsExec::Exec '"$INSTDIR\mdif.exe" agent start --after-install'
  Pop $0
  ${If} $0 == 2
  ${AndIf} $4 == 0
    nsExec::Exec '"$INSTDIR\mdif.exe" agent start'
    Pop $0
  ${EndIf}
  Abort "${message}"
!macroend

!macro NSIS_HOOK_PREINSTALL
  ; An update (or the same version again): mdif is already here. Before any
  ; file is copied: the watcher has ended, or nothing is (Windows would refuse
  ; to replace mdif-watch.exe after the app was, leaving the two versions
  ; mixed). `agent stop` waits 20 s for it, and fails if it's still running
  ; (in a long sample: the request stays for it); three times, then the
  ; install stops. One that can't run at all ("error") has nothing to stop.
  ; --for-install: the stop is the installer's (a user's stays theirs), so
  ; one this install leaves behind, if it fails, isn't taken for the user's.
  ; An mdif from before it refuses the option (clap's usage error, 2: its own
  ; errors are 1): asked plainly, and the marker it writes (beside the
  ; history: watch_cmd.rs's default_db) said to be the installer's, as
  ; --for-install would, unless one was there already.
  ${If} ${FileExists} "$INSTDIR\mdif.exe"
    ; The app closed first, before the watcher is touched (Tauri's own
    ; check, made again after this: asked, or closed in passive mode; one
    ; declined stops the install with nothing changed), so its monitor
    ; starts no mdif, nor the watcher, meanwhile.
    !insertmacro CheckIfAppIsRunning "$INSTDIR\${MAINBINARYNAME}.exe" "${PRODUCTNAME}"
    StrCpy $1 0
    SetShellVarContext current
    StrCpy $3 "$LOCALAPPDATA\MyDiskIsFull\history.sqlite3.stopped"
    ; Before any try: whether the user's was there before this install (a
    ; retry's plain stop writes the user's text over the installer's again;
    ; one an earlier install left says it's the installer's).
    StrCpy $4 0
    ${If} ${FileExists} $3
      StrCpy $4 1
      ClearErrors
      FileOpen $2 $3 r
      ${IfNot} ${Errors}
        FileRead $2 $5
        FileClose $2
        ; agent_windows.rs's INSTALLER_STOP
        ${If} $5 == "stopped by the installer"
          StrCpy $4 0
        ${EndIf}
      ${EndIf}
    ${EndIf}
    ${Do}
      nsExec::Exec '"$INSTDIR\mdif.exe" agent stop --for-install'
      Pop $0
      ${If} $0 == 2
        nsExec::Exec '"$INSTDIR\mdif.exe" agent stop'
        Pop $0
        ${If} $4 == 0
        ${AndIf} ${FileExists} $3
          ClearErrors
          FileOpen $2 $3 w
          ${IfNot} ${Errors}
            ; agent_windows.rs's INSTALLER_STOP
            FileWrite $2 "stopped by the installer"
            FileClose $2
          ${EndIf}
        ${EndIf}
      ${EndIf}
      ${If} $0 == 0
      ${OrIf} $0 == "error"
        ${Break}
      ${EndIf}
      IntOp $1 $1 + 1
      ${If} $1 >= 3
        !insertmacro MdifGiveUp "${PRODUCTNAME}'s watcher didn't stop, so nothing was changed. Install again in a minute."
      ${EndIf}
    ${Loop}
    ; Whatever that said ("error": mdif couldn't be run), nothing of mdif's
    ; still runs: not the watcher, nor mdif in a terminal (its user's to
    ; end, not the installer's), nor one the app started. A minute, then
    ; the install stops.
    StrCpy $1 0
    ${Do}
      StrCpy $5 0
      !insertmacro MdifInUse "$INSTDIR\mdif-watch.exe" $5
      !insertmacro MdifInUse "$INSTDIR\mdif.exe" $5
      ${If} $5 == 0
        ${Break}
      ${EndIf}
      IntOp $1 $1 + 1
      ${If} $1 >= 30
        !insertmacro MdifGiveUp "mdif (the watcher, or a command in a terminal) is still running, so nothing was changed. Install again once it has ended."
      ${EndIf}
      Sleep 2000
    ${Loop}
  ${EndIf}
!macroend

!macro NSIS_HOOK_POSTINSTALL
  ; Started again as sign-in would start it: not if it isn't set up, nor if
  ; it's switched off in Task Manager's Startup apps, nor if the user stopped
  ; it (`mdif agent stop`).
  nsExec::Exec '"$INSTDIR\mdif.exe" agent start --after-install'
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
