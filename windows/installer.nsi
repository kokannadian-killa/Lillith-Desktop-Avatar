; Lillith for Windows: NSIS installer (built by windows/build.sh).
;
;   makensis -DVERSION=0.0.1 -DSTAGE=<staged folder> -DOUTFILE=<setup.exe>
;            -DICON=windows/lillith.ico -DICONS_NSH=<icons.nsh>
;            [-DVOICE=<voice payload folder>] windows/installer.nsi
;
; -DVOICE (windows/build.sh sets it from VOICE_PAYLOAD=lillith-voice-win-<id>.zip)
; adds "Lillith's voice": Ko's cloned voice + the p1 fallback + offline speech
; recognition (tools/lillith-voice/windows/build-voice-win.sh builds it), installed
; per-user to %LOCALAPPDATA%\Lillith\voice and started at sign-in from
; HKCU\...\Run (no admin, no UAC). It serves \\.\pipe\lillith-voice only.
;
; Per-user install (no admin): %LOCALAPPDATA%\Programs\Lillith. Start menu
; shortcuts, a desktop shortcut (on by default, untickable), an optional
; sign-in (autostart) shortcut, a first-run
; "Lily's brain" page that points config.toml at a GGUF + llama-server.exe
; already on this PC (runs `lillith.exe setup`), an "App icon" page (the 15
; icons in icons\*.ico; shortcuts point at the chosen one; Lillith Settings >
; Icon changes it later), and an uninstaller.
; Nothing is downloaded, ever: no models, no runtimes, no update checks.
;
; Silent:  Lillith-setup.exe /S [/GGUF=C:\models\x.gguf] [/LLAMASERVER=C:\llama.cpp\llama-server.exe]
;          [/AUTOSTART] [/NODESKTOP] [/NOVOICE] [/ICON=<id>] [/D=C:\path\to\Lillith]   (/D must be last)
;          /ICON= takes an id from branding/icons.toml (e.g. glitch); default:
;          keep the configured icon (new installs: phantomcore-red).
;          The desktop shortcut is created by default (also with /S); /NODESKTOP skips it.

; 64-bit installer (Lillith is x64-only; also runs under 64-bit-only Wine).
Target amd64-unicode
ManifestDPIAware true
SetCompressor /SOLID lzma

!ifndef VERSION
  !error "pass -DVERSION=x.y.z"
!endif
!ifndef STAGE
  !error "pass -DSTAGE=<staged folder> (windows/build.sh does this)"
!endif
!ifndef OUTFILE
  !define OUTFILE "Lillith-${VERSION}-setup.exe"
!endif
!ifndef ICON
  !define ICON "lillith.ico"
!endif
!ifndef ICONS_NSH
  !error "pass -DICONS_NSH=<icons.nsh> (windows/build.sh generates it)"
!endif

!define APP      "Lillith"
!define UNINST   "Uninstall Lillith.exe"
!define REG_APP  "Software\Lillith"
!define REG_UNINST "Software\Microsoft\Windows\CurrentVersion\Uninstall\Lillith"
!define REG_RUN    "Software\Microsoft\Windows\CurrentVersion\Run"
; Voice payload home. NB: Windows paths are case-insensitive, so this is inside
; the app's own %LOCALAPPDATA%\lillith folder (chat history, log): never RMDir /r it.
!define VOICEDIR   "$LOCALAPPDATA\Lillith\voice"
!define VOICE_RUN  '"$LOCALAPPDATA\Lillith\voice\python\pythonw.exe" -m lillith_voice.winlaunch'

!include MUI2.nsh
!include nsDialogs.nsh
!include LogicLib.nsh
!include FileFunc.nsh
!include Sections.nsh

Name "${APP}"
OutFile "${OUTFILE}"
InstallDir "$LOCALAPPDATA\Programs\Lillith"
InstallDirRegKey HKCU "${REG_APP}" "InstallDir"
RequestExecutionLevel user
BrandingText "${APP} ${VERSION}"

VIProductVersion "${VERSION}.0"
VIAddVersionKey "ProductName" "${APP}"
VIAddVersionKey "ProductVersion" "${VERSION}"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "FileDescription" "${APP} desktop companion installer"
VIAddVersionKey "LegalCopyright" "MIT (code); see LICENSE-ASSETS.txt for the art"

Var Gguf
Var Server
Var hGguf
Var hServer
Var IconId     ; chosen icon id, "" = keep the configured one
Var IconName
Var hIcon

!include "${ICONS_NSH}"

!define MUI_ICON   "${ICON}"
!define MUI_UNICON "${ICON}"
!define MUI_ABORTWARNING
!define MUI_COMPONENTSPAGE_SMALLDESC

!define MUI_WELCOMEPAGE_TITLE "Install Lillith"
!define MUI_WELCOMEPAGE_TEXT "Lillith is a desktop companion who lives on top of your windows and taskbar.$\r$\n$\r$\nThis installs her for your Windows account only (no admin needed). Her chat brain is optional and fully local: a GGUF model run by llama.cpp's llama-server on this PC, talking over 127.0.0.1 only. The installer downloads nothing.$\r$\n$\r$\nClick Next to continue."
!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_LICENSE "${STAGE}/LICENSE.txt"
!insertmacro MUI_PAGE_COMPONENTS
!insertmacro MUI_PAGE_DIRECTORY
Page custom IconPage IconLeave
!insertmacro MUI_PAGE_INSTFILES
Page custom BrainPage BrainLeave
!define MUI_FINISHPAGE_RUN "$INSTDIR\lillith.exe"
!define MUI_FINISHPAGE_RUN_TEXT "Start Lillith now"
!define MUI_FINISHPAGE_SHOWREADME "$INSTDIR\INSTALL-windows.txt"
!define MUI_FINISHPAGE_SHOWREADME_TEXT "Show the setup notes"
!define MUI_FINISHPAGE_SHOWREADME_NOTCHECKED
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "English"

; ── helpers ───────────────────────────────────────────────────────────

!macro KillRunning
  ; Close a running Lillith (and the llama-server she started) so files
  ; can be replaced. /T = whole process tree; only this user's processes.
  nsExec::Exec '"$SYSDIR\taskkill.exe" /IM lillith.exe /T /F'
  Pop $0
  nsExec::Exec '"$SYSDIR\taskkill.exe" /IM lillith-settings.exe /F'
  Pop $0
  nsExec::Exec '"$SYSDIR\taskkill.exe" /IM lillith-admin.exe /T /F'
  Pop $0
  Sleep 300
!macroend

; Asks the bundled voice daemon to quit (its own --stop: a "shutdown" over the
; pipe), so its files can be replaced. Never a taskkill on python*.exe: that
; would hit the user's other Pythons.
!macro StopVoice
  ${If} ${FileExists} "${VOICEDIR}\python\python.exe"
    nsExec::Exec '"${VOICEDIR}\python\python.exe" -m lillith_voice.winlaunch --stop'
    Pop $0
  ${EndIf}
!macroend

; Runs `lillith.exe setup` with $Gguf / $Server. Leaves exit code in $1,
; output in $2.
Function RunSetup
  StrCpy $0 '"$INSTDIR\lillith.exe" setup'
  ${If} $Gguf != ""
    StrCpy $0 '$0 --gguf "$Gguf"'
  ${EndIf}
  ${If} $Server != ""
    StrCpy $0 '$0 --llama-server "$Server"'
  ${EndIf}
  DetailPrint "Saving the local model choice..."
  nsExec::ExecToStack $0
  Pop $1
  Pop $2
  DetailPrint "$2"
FunctionEnd

; $R9 = the shortcut icon: icons\<chosen>.ico, else the default's.
Function ShortcutIcon
  StrCpy $R9 "$INSTDIR\icons\${ICON_DEFAULT}.ico"
  ${If} $IconId != ""
    StrCpy $R9 "$INSTDIR\icons\$IconId.ico"
  ${EndIf}
  ${IfNot} ${FileExists} "$R9"
    StrCpy $R9 "$INSTDIR\lillith.ico"
  ${EndIf}
FunctionEnd

; Pre-fills llama-server.exe from the folders Lillith also searches.
Function GuessServer
  ${If} $Server != ""
    Return
  ${EndIf}
  ${If} ${FileExists} "$INSTDIR\llama.cpp\llama-server.exe"
    StrCpy $Server "$INSTDIR\llama.cpp\llama-server.exe"
  ${ElseIf} ${FileExists} "$LOCALAPPDATA\Programs\llama.cpp\llama-server.exe"
    StrCpy $Server "$LOCALAPPDATA\Programs\llama.cpp\llama-server.exe"
  ${ElseIf} ${FileExists} "C:\llama.cpp\llama-server.exe"
    StrCpy $Server "C:\llama.cpp\llama-server.exe"
  ${EndIf}
FunctionEnd

; ── sections ──────────────────────────────────────────────────────────

Section "Lillith (required)" SecMain
  SectionIn RO
  !insertmacro KillRunning
  SetOutPath "$INSTDIR"
  ; Replace the art cleanly on upgrade (only our own folder).
  RMDir /r "$INSTDIR\assets"
  RMDir /r "$INSTDIR\icons"
  File /r "${STAGE}/*"
  WriteUninstaller "$INSTDIR\${UNINST}"

  WriteRegStr HKCU "${REG_APP}" "InstallDir" "$INSTDIR"
  WriteRegStr HKCU "${REG_UNINST}" "DisplayName" "${APP}"
  WriteRegStr HKCU "${REG_UNINST}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${REG_UNINST}" "Publisher" "Lillith"
  WriteRegStr HKCU "${REG_UNINST}" "DisplayIcon" "$INSTDIR\lillith.ico"
  WriteRegStr HKCU "${REG_UNINST}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${REG_UNINST}" "UninstallString" '"$INSTDIR\${UNINST}"'
  WriteRegStr HKCU "${REG_UNINST}" "QuietUninstallString" '"$INSTDIR\${UNINST}" /S'
  WriteRegDWORD HKCU "${REG_UNINST}" "NoModify" 1
  WriteRegDWORD HKCU "${REG_UNINST}" "NoRepair" 1
  ${GetSize} "$INSTDIR" "/S=0K" $0 $1 $2
  IntFmt $0 "0x%08X" $0
  WriteRegDWORD HKCU "${REG_UNINST}" "EstimatedSize" "$0"

  CreateDirectory "$SMPROGRAMS\Lillith"
  Call ShortcutIcon
  CreateShortcut "$SMPROGRAMS\Lillith\Lillith.lnk" "$INSTDIR\lillith.exe" "" "$R9" 0 SW_SHOWNORMAL "" "Start Lillith"
  CreateShortcut "$SMPROGRAMS\Lillith\Lillith Settings.lnk" "$INSTDIR\lillith-settings.exe" "" "$R9" 0 SW_SHOWNORMAL "" "Lillith's settings and local-model setup"
  CreateShortcut "$SMPROGRAMS\Lillith\Uninstall Lillith.lnk" "$INSTDIR\${UNINST}"
  ; Optional shortcuts are re-created below only if still ticked.
  Delete "$SMSTARTUP\Lillith.lnk"
  Delete "$DESKTOP\Lillith.lnk"
SectionEnd

Section "Desktop shortcut" SecDesktop
  Call ShortcutIcon
  CreateShortcut "$DESKTOP\Lillith.lnk" "$INSTDIR\lillith.exe" "" "$R9"
SectionEnd

Section /o "Start Lillith when I sign in" SecAutostart
  Call ShortcutIcon
  CreateShortcut "$SMSTARTUP\Lillith.lnk" "$INSTDIR\lillith.exe" "" "$R9"
SectionEnd

!ifdef VOICE
; Lillith's own voice: everything is in the installer (Python, wheels, models,
; ffmpeg); nothing is downloaded. Per-user, no admin.
Section "Lillith's voice (Ko's cloned voice, offline)" SecVoice
  !insertmacro StopVoice
  Sleep 300
  ; Replace the bundled parts cleanly; voices\ (your own clones) and logs\ stay.
  RMDir /r "${VOICEDIR}\python"
  RMDir /r "${VOICEDIR}\lib"
  RMDir /r "${VOICEDIR}\models"
  RMDir /r "${VOICEDIR}\bin"
  RMDir /r "${VOICEDIR}\tests"
  RMDir /r "${VOICEDIR}\licenses"
  SetOutPath "${VOICEDIR}"
  File /r "${VOICE}/*"
  WriteRegStr HKCU "${REG_APP}" "VoiceDir" "${VOICEDIR}"
  ; Per-user autostart of the voice daemon (hidden pythonw; exits at once if one
  ; is already running). Lillith finds it on \\.\pipe\lillith-voice by default.
  WriteRegStr HKCU "${REG_RUN}" "LillithVoice" '${VOICE_RUN}'
  CreateDirectory "$SMPROGRAMS\Lillith"
  CreateShortcut "$SMPROGRAMS\Lillith\Lillith voice - microphone setup.lnk" "${VOICEDIR}\lillith-listen.cmd" "--setup" "$INSTDIR\lillith.ico" 0 SW_SHOWNORMAL "" "Pick and test the microphone Lillith listens to"
  ; Sign-in autostart of the app: go through the voice launcher, which waits until
  ; the daemon answers (the app probes it once at startup, else falls back to SAPI).
  ${If} ${SectionIsSelected} ${SecAutostart}
    Call ShortcutIcon
    Delete "$SMSTARTUP\Lillith.lnk"
    CreateShortcut "$SMSTARTUP\Lillith.lnk" "${VOICEDIR}\python\pythonw.exe" '-m lillith_voice.winlaunch --start-app "$INSTDIR\lillith.exe"' "$R9" 0 SW_SHOWNORMAL "" "Lillith (waits for her voice at sign-in)"
  ${EndIf}
  ; Start it now so she talks on the first run.
  DetailPrint "Starting Lillith's voice..."
  Exec '${VOICE_RUN}'
SectionEnd
!endif

; App icon: a picked one is saved to config.toml; otherwise the configured
; icon (an earlier pick, or the default) is re-applied to the shortcuts.
Section "-AppIcon"
  ${If} $IconId != ""
    DetailPrint "App icon: $IconName"
    nsExec::ExecToLog '"$INSTDIR\lillith.exe" icon $IconId --no-apply'
    Pop $0
  ${Else}
    nsExec::ExecToLog '"$INSTDIR\lillith.exe" icon --reapply'
    Pop $0
  ${EndIf}
SectionEnd

; Silent installs: /GGUF= and /LLAMASERVER= replace the brain page.
Section "-SilentBrain"
  ${If} ${Silent}
    ${If} $Gguf != ""
    ${OrIf} $Server != ""
      Call RunSetup
    ${EndIf}
  ${EndIf}
SectionEnd

; (after the sections: uses ${SecAutostart} and ${SecDesktop})
Function .onInit
  ${GetParameters} $R0
  ClearErrors
  ${GetOptions} $R0 "/GGUF=" $Gguf
  ClearErrors
  ${GetOptions} $R0 "/LLAMASERVER=" $Server
  ClearErrors
  ${GetOptions} $R0 "/AUTOSTART" $R1
  ${IfNot} ${Errors}
    !insertmacro SelectSection ${SecAutostart}
  ${EndIf}
  ClearErrors
  ${GetOptions} $R0 "/NODESKTOP" $R1
  ${IfNot} ${Errors}
    !insertmacro UnselectSection ${SecDesktop}
  ${EndIf}
!ifdef VOICE
  ClearErrors
  ${GetOptions} $R0 "/NOVOICE" $R1
  ${IfNot} ${Errors}
    !insertmacro UnselectSection ${SecVoice}
  ${EndIf}
!endif
  ClearErrors
  ${GetOptions} $R0 "/ICON=" $IconId
  ${If} ${Errors}
    StrCpy $IconId ""
  ${EndIf}
  ${If} $IconId != ""
    Call IconNameOfId
    ${If} $IconName == ""
      StrCpy $IconId ""   ; unknown id: keep the configured icon
    ${EndIf}
  ${EndIf}
FunctionEnd

!insertmacro MUI_FUNCTION_DESCRIPTION_BEGIN
  !insertmacro MUI_DESCRIPTION_TEXT ${SecMain} "Lillith, Lillith Settings, her animations, and Start menu shortcuts."
  !insertmacro MUI_DESCRIPTION_TEXT ${SecDesktop} "A Lillith shortcut on the desktop."
  !insertmacro MUI_DESCRIPTION_TEXT ${SecAutostart} "Start Lillith automatically when you sign in to Windows."
!ifdef VOICE
  !insertmacro MUI_DESCRIPTION_TEXT ${SecVoice} "Her own voice (Ko's cloned voice, a backup voice) and offline speech recognition for the mic. All local, no admin; starts hidden when you sign in. Installs to %LOCALAPPDATA%\Lillith\voice."
!endif
!insertmacro MUI_FUNCTION_DESCRIPTION_END

; ── icon page ─────────────────────────────────────────────────────────

!define KEEP_ICON "Keep the current icon (new installs: Phantomcore Red)"

Function IconPage
  !insertmacro MUI_HEADER_TEXT "App icon" "Pick the icon for Lillith's shortcuts."
  nsDialogs::Create 1018
  Pop $0
  ${If} $0 == error
    Abort
  ${EndIf}
  ${NSD_CreateLabel} 0 0 100% 24u "Lillith comes with 15 icons. The Start menu and desktop shortcuts use the one you pick. You can change it any time in Lillith Settings > Icon (or right-click her > Icon...)."
  Pop $0
  ${NSD_CreateDropList} 0 30u 70% 160u ""
  Pop $hIcon
  ${NSD_CB_AddString} $hIcon "${KEEP_ICON}"
  !insertmacro FillIconList $hIcon
  ${If} $IconName != ""
    ${NSD_CB_SelectString} $hIcon "$IconName"
  ${Else}
    ${NSD_CB_SelectString} $hIcon "${KEEP_ICON}"
  ${EndIf}
  nsDialogs::Show
FunctionEnd

Function IconLeave
  ${NSD_GetText} $hIcon $IconName
  ${If} $IconName == "${KEEP_ICON}"
    StrCpy $IconName ""
  ${EndIf}
  Call IconIdOfName
FunctionEnd

; ── first-run page: local model ───────────────────────────────────────

Function BrainPage
  Call GuessServer
  !insertmacro MUI_HEADER_TEXT "Lily's brain (optional, local only)" "Point Lillith at a model already on this PC."
  nsDialogs::Create 1018
  Pop $0
  ${If} $0 == error
    Abort
  ${EndIf}

  ${NSD_CreateLabel} 0 0 100% 34u "Lillith chats through llama.cpp's llama-server running on this PC, reached at 127.0.0.1 only. Nothing is downloaded and nothing leaves this computer. Pick files you already have, or leave both empty and set them later in Lillith Settings > Brain."
  Pop $0

  ${NSD_CreateLabel} 0 40u 100% 10u "Model file (.gguf):"
  Pop $0
  ${NSD_CreateFileRequest} 0 52u 78% 13u "$Gguf"
  Pop $hGguf
  ${NSD_CreateBrowseButton} 80% 51u 20% 15u "Browse..."
  Pop $0
  ${NSD_OnClick} $0 BrowseGguf

  ${NSD_CreateLabel} 0 74u 100% 10u "llama-server.exe (from a llama.cpp Windows release, with its DLLs beside it):"
  Pop $0
  ${NSD_CreateFileRequest} 0 86u 78% 13u "$Server"
  Pop $hServer
  ${NSD_CreateBrowseButton} 80% 85u 20% 15u "Browse..."
  Pop $0
  ${NSD_OnClick} $0 BrowseServer

  ${NSD_CreateLabel} 0 108u 100% 30u "Lillith starts llama-server herself (hidden, 127.0.0.1) when she starts and stops it when she quits. Settings live in %APPDATA%\lillith\config.toml."
  Pop $0

  nsDialogs::Show
FunctionEnd

Function BrowseGguf
  ${NSD_GetText} $hGguf $0
  nsDialogs::SelectFileDialog open "$0" "GGUF models (*.gguf)|*.gguf|All files (*.*)|*.*"
  Pop $0
  ${If} $0 != ""
    ${NSD_SetText} $hGguf $0
  ${EndIf}
FunctionEnd

Function BrowseServer
  ${NSD_GetText} $hServer $0
  nsDialogs::SelectFileDialog open "$0" "llama-server.exe|llama-server.exe|Programs (*.exe)|*.exe"
  Pop $0
  ${If} $0 != ""
    ${NSD_SetText} $hServer $0
  ${EndIf}
FunctionEnd

Function BrainLeave
  ${NSD_GetText} $hGguf $Gguf
  ${NSD_GetText} $hServer $Server
  ${If} $Gguf == ""
  ${AndIf} $Server == ""
    Return
  ${EndIf}
  Call RunSetup
  ${If} $1 != 0
    MessageBox MB_ICONEXCLAMATION|MB_OK "Lillith couldn't use that:$\r$\n$\r$\n$2$\r$\nFix the path, or clear both boxes to skip."
    Abort
  ${EndIf}
FunctionEnd

; ── uninstaller ───────────────────────────────────────────────────────

Function un.onInit
  SetShellVarContext current
FunctionEnd

Section "Uninstall"
  !insertmacro KillRunning
  ; Lillith's voice (whether or not this installer added it)
  !insertmacro StopVoice
  DeleteRegValue HKCU "${REG_RUN}" "LillithVoice"
  Delete "$SMPROGRAMS\Lillith\Lillith voice - microphone setup.lnk"
  RMDir /r "${VOICEDIR}\python"
  RMDir /r "${VOICEDIR}\lib"
  RMDir /r "${VOICEDIR}\models"
  RMDir /r "${VOICEDIR}\bin"
  RMDir /r "${VOICEDIR}\tests"
  RMDir /r "${VOICEDIR}\licenses"
  RMDir /r "${VOICEDIR}\logs"
  Delete "${VOICEDIR}\lillith-voiced.cmd"
  Delete "${VOICEDIR}\lillith-say.cmd"
  Delete "${VOICEDIR}\lillith-listen.cmd"
  Delete "${VOICEDIR}\*.md"
  Delete "${VOICEDIR}\voice-win.lock.json"
  Delete "${VOICEDIR}\PAYLOAD_ID"
  Delete "${VOICEDIR}\SHA256SUMS"
  RMDir "${VOICEDIR}"   ; only if empty (voices\ = your own clones stay unless you delete settings)

  Delete "$SMPROGRAMS\Lillith\Lillith.lnk"
  Delete "$SMPROGRAMS\Lillith\Lillith Settings.lnk"
  Delete "$SMPROGRAMS\Lillith\Uninstall Lillith.lnk"
  RMDir "$SMPROGRAMS\Lillith"
  Delete "$SMSTARTUP\Lillith.lnk"
  Delete "$DESKTOP\Lillith.lnk"

  ; Only what the installer put there (a llama.cpp folder or models you
  ; dropped next to Lillith are left alone).
  Delete "$INSTDIR\lillith.exe"
  Delete "$INSTDIR\lillith-settings.exe"
  Delete "$INSTDIR\lillith-admin.exe"
  Delete "$INSTDIR\lillith.ico"
  RMDir /r "$INSTDIR\icons"
  Delete "$INSTDIR\INSTALL-windows.txt"
  Delete "$INSTDIR\LICENSE.txt"
  Delete "$INSTDIR\LICENSE-ASSETS.txt"
  Delete "$INSTDIR\config.example.toml"
  RMDir /r "$INSTDIR\assets"
  Delete "$INSTDIR\${UNINST}"
  RMDir "$INSTDIR"

  DeleteRegKey HKCU "${REG_UNINST}"
  DeleteRegKey HKCU "${REG_APP}"

  MessageBox MB_YESNO|MB_ICONQUESTION|MB_DEFBUTTON2 "Also delete your Lillith settings, chat history and log?$\r$\n$\r$\n($APPDATA\lillith\config.toml, Modelfile.lily, $LOCALAPPDATA\lillith\chat_history.json, lillith.log, the voice's mic settings and any voices you cloned yourself)$\r$\n$\r$\nYour GGUF models and llama.cpp are never touched." /SD IDNO IDNO keep
    Delete "$APPDATA\lillith\config.toml"
    Delete "$APPDATA\lillith\config.toml.bak"
    Delete "$APPDATA\lillith\Modelfile.lily"
    RMDir "$APPDATA\lillith"
    RMDir /r "$APPDATA\lillith-voice"   ; voice: mic.json, presets.json
    RMDir /r "${VOICEDIR}\voices"
    RMDir "${VOICEDIR}"
    Delete "$LOCALAPPDATA\lillith\chat_history.json"
    Delete "$LOCALAPPDATA\lillith\lillith.log"
    RMDir "$LOCALAPPDATA\lillith"
  keep:
SectionEnd
