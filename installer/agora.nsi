; Agora installer for Windows. Installs for the current user only (no
; admin needed) into %LOCALAPPDATA%\Programs\Agora.
;
; Build (from the repo root, after the Windows build):
;   makensis -DVERSION=0.6.0 -DSRC=<folder with agora.exe and WebView2Loader.dll> installer/agora.nsi
; The installer lands in that same folder.

Unicode true
!include "MUI2.nsh"
!include "LogicLib.nsh"

!ifndef VERSION
  !error "Pass -DVERSION=x.y.z"
!endif
!ifndef SRC
  !error "Pass -DSRC=<folder with agora.exe>"
!endif

!define APP "Agora"
!define UNINST_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\Agora"
!define RUN_KEY "Software\Microsoft\Windows\CurrentVersion\Run"

Name "${APP}"
OutFile "${SRC}\Agora-${VERSION}-setup.exe"
InstallDir "$LOCALAPPDATA\Programs\Agora"
InstallDirRegKey HKCU "${UNINST_KEY}" "InstallLocation"
RequestExecutionLevel user
SetCompressor /SOLID lzma
BrandingText "Agora ${VERSION} beta"

VIProductVersion "${VERSION}.0"
VIAddVersionKey "ProductName" "${APP}"
VIAddVersionKey "ProductVersion" "${VERSION}"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "FileDescription" "Agora installer"
VIAddVersionKey "LegalCopyright" "Copyright (c) 2026 overture-sys. MIT License."

!define MUI_ICON "..\src-tauri\icons\icon.ico"
!define MUI_UNICON "..\src-tauri\icons\icon.ico"
!define MUI_ABORTWARNING
!define MUI_WELCOMEPAGE_TEXT "Agora is a free, open-source app for Stream Dock devices that runs your Stream Dock plugins.$\r$\n$\r$\nThis is a beta, tested on the Stream Dock M18. Agora and VSD Craft can't use the dock at the same time, so close VSD Craft before you start Agora.$\r$\n$\r$\nNo administrator rights are needed: Agora installs for you only."
!define MUI_FINISHPAGE_RUN "$INSTDIR\agora.exe"
!define MUI_FINISHPAGE_RUN_TEXT "Start Agora now"

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_LICENSE "..\LICENSE"
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

; A running Agora holds agora.exe open. Quitting from the tray hands the
; dock back cleanly, so ask for that first; close it if the user agrees.
!macro CloseAgora
  nsExec::ExecToStack 'cmd /c tasklist /FI "IMAGENAME eq agora.exe" /NH | find /I "agora.exe"'
  Pop $0
  Pop $1
  ${If} $0 == 0
    MessageBox MB_OKCANCEL|MB_ICONINFORMATION "Agora is running. Quit it from its tray icon, then click OK.$\r$\n$\r$\nIf it is still running, clicking OK closes it." IDOK +2
    Abort
    nsExec::Exec 'taskkill /IM agora.exe /F'
    Sleep 1000
  ${EndIf}
!macroend

Section "Agora" SecMain
  !insertmacro CloseAgora
  SetOutPath "$INSTDIR"
  File "${SRC}\agora.exe"
  File "${SRC}\WebView2Loader.dll"
  File "..\LICENSE"
  File "..\README.md"

  CreateShortCut "$SMPROGRAMS\Agora.lnk" "$INSTDIR\agora.exe"

  ; If Agora was set to start with Windows from another folder, point it here.
  ReadRegStr $0 HKCU "${RUN_KEY}" "Agora"
  ${If} $0 != ""
    WriteRegStr HKCU "${RUN_KEY}" "Agora" '"$INSTDIR\agora.exe" --tray'
  ${EndIf}

  WriteUninstaller "$INSTDIR\uninstall.exe"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayName" "Agora"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${UNINST_KEY}" "Publisher" "overture-sys"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayIcon" "$INSTDIR\agora.exe"
  WriteRegStr HKCU "${UNINST_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${UNINST_KEY}" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegStr HKCU "${UNINST_KEY}" "URLInfoAbout" "https://gitlab.com/overture-group1/agora"
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoRepair" 1
SectionEnd

; Keys, profiles, settings and plugins in %APPDATA%\Agora are kept, so a
; reinstall picks up where you left off.
Section "Uninstall"
  !insertmacro CloseAgora
  Delete "$INSTDIR\agora.exe"
  Delete "$INSTDIR\WebView2Loader.dll"
  Delete "$INSTDIR\LICENSE"
  Delete "$INSTDIR\README.md"
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"
  Delete "$SMPROGRAMS\Agora.lnk"
  DeleteRegValue HKCU "${RUN_KEY}" "Agora"
  DeleteRegKey HKCU "${UNINST_KEY}"
SectionEnd
