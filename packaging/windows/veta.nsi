; Veta installer (NSIS). Built by scripts/release.sh:
;   makensis -DVERSION=0.1.0 -DBUILD=52 -DSRC=<dir with exes> -DOUT=<setup.exe> veta.nsi
; Installs for the current user (no admin rights) into
; %LOCALAPPDATA%\Programs\Veta.

Unicode true
!include "MUI2.nsh"
!include "WordFunc.nsh"

!ifndef VERSION
  !error "VERSION is not defined"
!endif
!ifndef BUILD
  !define BUILD 0
!endif

!define APP "Veta"
!define UNINSTALL_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\Veta"

Name "${APP} ${VERSION}"
OutFile "${OUT}"
InstallDir "$LOCALAPPDATA\Programs\Veta"
InstallDirRegKey HKCU "Software\Veta" "InstallDir"
RequestExecutionLevel user
SetCompressor /SOLID lzma
ManifestDPIAware true

VIProductVersion "${VERSION}.${BUILD}"
VIAddVersionKey "ProductName" "${APP}"
VIAddVersionKey "ProductVersion" "${VERSION} (build ${BUILD})"
VIAddVersionKey "FileVersion" "${VERSION}.${BUILD}"
VIAddVersionKey "FileDescription" "${APP} installer"
VIAddVersionKey "LegalCopyright" "Apache-2.0"

!define MUI_ICON "..\icons\veta.ico"
!define MUI_UNICON "..\icons\veta.ico"
!define MUI_ABORTWARNING
!define MUI_COMPONENTSPAGE_NODESC
!define MUI_FINISHPAGE_RUN "$INSTDIR\veta.exe"
!define MUI_FINISHPAGE_RUN_TEXT "Start Veta"

!insertmacro MUI_PAGE_LICENSE "${SRC}\LICENSE"
!insertmacro MUI_PAGE_COMPONENTS
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

Section "Veta (required)" SecApp
  SectionIn RO
  SetShellVarContext current
  SetOutPath "$INSTDIR"
  ; If Veta is running, Windows refuses to replace veta.exe: NSIS then
  ; asks to close it and retry.
  File "${SRC}\veta.exe"
  File "${SRC}\veta-cli.exe"
  File "${SRC}\README.md"
  File "${SRC}\LICENSE"
  File "${SRC}\CHANGELOG.md"
  WriteUninstaller "$INSTDIR\uninstall.exe"
  CreateShortcut "$SMPROGRAMS\Veta.lnk" "$INSTDIR\veta.exe"

  WriteRegStr HKCU "Software\Veta" "InstallDir" "$INSTDIR"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayName" "${APP}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "Publisher" "Veta"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayIcon" "$INSTDIR\veta.exe"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegStr HKCU "${UNINSTALL_KEY}" "QuietUninstallString" '"$INSTDIR\uninstall.exe" /S'
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoRepair" 1
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "EstimatedSize" 60000
SectionEnd

Section "Add veta-cli to PATH" SecPath
  ReadRegStr $0 HKCU "Environment" "Path"
  ; Already there?
  ${WordFind} "$0" "$INSTDIR" "E+1{" $1
  IfErrors 0 path_done
  ; NSIS strings are limited; never write back a truncated PATH.
  StrLen $1 $0
  IntCmp $1 900 0 0 path_too_long
  StrCmp $0 "" 0 +3
    StrCpy $0 "$INSTDIR"
    Goto path_write
  StrCpy $0 "$0;$INSTDIR"
  path_write:
  WriteRegExpandStr HKCU "Environment" "Path" "$0"
  WriteRegDWORD HKCU "Software\Veta" "AddedToPath" 1
  SendMessage ${HWND_BROADCAST} ${WM_SETTINGCHANGE} 0 "STR:Environment" /TIMEOUT=5000
  Goto path_done
  path_too_long:
  DetailPrint "Your PATH is too long to change safely; add $INSTDIR to it by hand to use veta-cli."
  path_done:
SectionEnd

Section "Uninstall"
  SetShellVarContext current
  ReadRegDWORD $0 HKCU "Software\Veta" "AddedToPath"
  IntCmp $0 1 0 unpath_done unpath_done
  ReadRegStr $0 HKCU "Environment" "Path"
  ${WordReplace} "$0" ";$INSTDIR" "" "+" $0
  ${WordReplace} "$0" "$INSTDIR;" "" "+" $0
  ${WordReplace} "$0" "$INSTDIR" "" "+" $0
  WriteRegExpandStr HKCU "Environment" "Path" "$0"
  SendMessage ${HWND_BROADCAST} ${WM_SETTINGCHANGE} 0 "STR:Environment" /TIMEOUT=5000
  unpath_done:

  Delete "$SMPROGRAMS\Veta.lnk"
  Delete "$INSTDIR\veta.exe"
  Delete "$INSTDIR\veta-cli.exe"
  Delete "$INSTDIR\README.md"
  Delete "$INSTDIR\LICENSE"
  Delete "$INSTDIR\CHANGELOG.md"
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"
  DeleteRegKey HKCU "${UNINSTALL_KEY}"
  DeleteRegKey HKCU "Software\Veta"
  ; Settings in %APPDATA%\veta are kept.
SectionEnd
