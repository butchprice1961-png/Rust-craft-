; RustCraft per-user installer (no administrator rights needed).
; Build: makensis /DVERSION=0.1.0 /DSRC=<dir with rustcraft.exe> /DOUT=<installer.exe> installer.nsi
Unicode true
!include "MUI2.nsh"

Name "RustCraft"
OutFile "${OUT}"
InstallDir "$LOCALAPPDATA\Programs\RustCraft"
InstallDirRegKey HKCU "Software\RustCraft" "InstallDir"
RequestExecutionLevel user
SetCompressor /SOLID lzma

VIProductVersion "${VERSION}.0"
VIAddVersionKey "ProductName" "RustCraft"
VIAddVersionKey "FileDescription" "RustCraft installer"
VIAddVersionKey "ProductVersion" "${VERSION}"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "LegalCopyright" "RustCraft contributors. Not affiliated with Mojang or Microsoft."

!define MUI_ICON "..\icons\rustcraft.ico"
!define MUI_UNICON "..\icons\rustcraft.ico"
!define MUI_FINISHPAGE_RUN "$INSTDIR\rustcraft.exe"
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

!define UNINST_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\RustCraft"

Section "RustCraft"
  SetOutPath "$INSTDIR"
  File "${SRC}\*.exe"
  File "..\..\LICENSE"
  WriteUninstaller "$INSTDIR\uninstall.exe"
  CreateDirectory "$SMPROGRAMS\RustCraft"
  CreateShortcut "$SMPROGRAMS\RustCraft\RustCraft.lnk" "$INSTDIR\rustcraft.exe"
  WriteRegStr HKCU "Software\RustCraft" "InstallDir" "$INSTDIR"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayName" "RustCraft"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${UNINST_KEY}" "Publisher" "RustCraft contributors"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayIcon" "$INSTDIR\rustcraft.exe"
  WriteRegStr HKCU "${UNINST_KEY}" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegStr HKCU "${UNINST_KEY}" "QuietUninstallString" '"$INSTDIR\uninstall.exe" /S'
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoRepair" 1
SectionEnd

Section "Uninstall"
  Delete "$INSTDIR\*.exe"
  Delete "$INSTDIR\LICENSE"
  RMDir "$INSTDIR"
  Delete "$SMPROGRAMS\RustCraft\RustCraft.lnk"
  RMDir "$SMPROGRAMS\RustCraft"
  DeleteRegKey HKCU "${UNINST_KEY}"
  DeleteRegKey HKCU "Software\RustCraft"
  ; Worlds, settings and downloads in %APPDATA%\RustCraft are kept unless
  ; the user explicitly asks (never in silent mode).
  IfSilent done
  MessageBox MB_YESNO|MB_DEFBUTTON2 "Also delete your RustCraft data (instances with their worlds, settings and downloaded game files) in $APPDATA\RustCraft?" IDNO done
  RMDir /r "$APPDATA\RustCraft"
  done:
SectionEnd
