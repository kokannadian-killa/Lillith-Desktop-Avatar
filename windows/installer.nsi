; Lillith for Windows: NSIS installer (built by windows/build.sh).
;
;   makensis -DVERSION=0.0.1 -DSTAGE=<staged folder> -DOUTFILE=<setup.exe>
;            -DICON=windows/lillith.ico windows/installer.nsi
;
; Per-user install (no admin): %LOCALAPPDATA%\Programs\Lillith. Start menu
; shortcuts, optional desktop + sign-in (autostart) shortcuts, a first-run
; "Lily's brain" page that points config.toml at a GGUF + llama-server.exe
; already on this PC (runs `lillith.exe setup`), and an uninstaller.
; Nothing is downloaded, ever: no models, no runtimes, no update checks.
;
; Silent:  Lillith-setup.exe /S [/GGUF=C:\models\x.gguf] [/LLAMASERVER=C:\llama.cpp\llama-server.exe]
;          [/AUTOSTART] [/D=C:\path\to\Lillith]   (/D must be last)

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

!define APP      "Lillith"
!define UNINST   "Uninstall Lillith.exe"
!define REG_APP  "Software\Lillith"
!define REG_UNINST "Software\Microsoft\Windows\CurrentVersion\Uninstall\Lillith"

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
  Sleep 300
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
  CreateShortcut "$SMPROGRAMS\Lillith\Lillith.lnk" "$INSTDIR\lillith.exe" "" "$INSTDIR\lillith.ico" 0 SW_SHOWNORMAL "" "Start Lillith"
  CreateShortcut "$SMPROGRAMS\Lillith\Lillith Settings.lnk" "$INSTDIR\lillith-settings.exe" "" "$INSTDIR\lillith.ico" 0 SW_SHOWNORMAL "" "Lillith's settings and local-model setup"
  CreateShortcut "$SMPROGRAMS\Lillith\Uninstall Lillith.lnk" "$INSTDIR\${UNINST}"
  ; Optional shortcuts are re-created below only if still ticked.
  Delete "$SMSTARTUP\Lillith.lnk"
  Delete "$DESKTOP\Lillith.lnk"
SectionEnd

Section /o "Desktop shortcut" SecDesktop
  CreateShortcut "$DESKTOP\Lillith.lnk" "$INSTDIR\lillith.exe" "" "$INSTDIR\lillith.ico"
SectionEnd

Section /o "Start Lillith when I sign in" SecAutostart
  CreateShortcut "$SMSTARTUP\Lillith.lnk" "$INSTDIR\lillith.exe" "" "$INSTDIR\lillith.ico"
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

; (after the sections: uses ${SecAutostart})
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
FunctionEnd

!insertmacro MUI_FUNCTION_DESCRIPTION_BEGIN
  !insertmacro MUI_DESCRIPTION_TEXT ${SecMain} "Lillith, Lillith Settings, her animations, and Start menu shortcuts."
  !insertmacro MUI_DESCRIPTION_TEXT ${SecDesktop} "A Lillith shortcut on the desktop."
  !insertmacro MUI_DESCRIPTION_TEXT ${SecAutostart} "Start Lillith automatically when you sign in to Windows."
!insertmacro MUI_FUNCTION_DESCRIPTION_END

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
  Delete "$INSTDIR\lillith.ico"
  Delete "$INSTDIR\INSTALL-windows.txt"
  Delete "$INSTDIR\LICENSE.txt"
  Delete "$INSTDIR\LICENSE-ASSETS.txt"
  Delete "$INSTDIR\config.example.toml"
  RMDir /r "$INSTDIR\assets"
  Delete "$INSTDIR\${UNINST}"
  RMDir "$INSTDIR"

  DeleteRegKey HKCU "${REG_UNINST}"
  DeleteRegKey HKCU "${REG_APP}"

  MessageBox MB_YESNO|MB_ICONQUESTION|MB_DEFBUTTON2 "Also delete your Lillith settings, chat history and log?$\r$\n$\r$\n($APPDATA\lillith\config.toml, Modelfile.lily, $LOCALAPPDATA\lillith\chat_history.json, lillith.log)$\r$\n$\r$\nYour GGUF models and llama.cpp are never touched." /SD IDNO IDNO keep
    Delete "$APPDATA\lillith\config.toml"
    Delete "$APPDATA\lillith\config.toml.bak"
    Delete "$APPDATA\lillith\Modelfile.lily"
    RMDir "$APPDATA\lillith"
    Delete "$LOCALAPPDATA\lillith\chat_history.json"
    Delete "$LOCALAPPDATA\lillith\lillith.log"
    RMDir "$LOCALAPPDATA\lillith"
  keep:
SectionEnd
