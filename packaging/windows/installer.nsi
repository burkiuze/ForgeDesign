; Forge Design — Windows setup wizard (NSIS / Modern UI 2)
; Build: makensis -DVERSION=0.1.0 -DSRC=<dir with forge.exe> installer.nsi

Unicode true
!include "MUI2.nsh"
!include "FileFunc.nsh"

!ifndef VERSION
  !define VERSION "0.1.0"
!endif
!ifndef SRC
  !define SRC "..\..\target\x86_64-pc-windows-gnu\release"
!endif

!define APP "Forge Design"
!define EXE "forge.exe"
!define REGKEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\ForgeDesign"

Name "${APP} ${VERSION}"
OutFile "ForgeDesign-${VERSION}-Setup.exe"
InstallDir "$PROGRAMFILES64\${APP}"
InstallDirRegKey HKLM "Software\ForgeDesign" "InstallDir"
RequestExecutionLevel admin
SetCompressor /SOLID lzma
BrandingText "${APP} — open-source image editor"

!define MUI_ICON "forge.ico"
!define MUI_UNICON "forge.ico"
!define MUI_WELCOMEFINISHPAGE_BITMAP "wizard.bmp"
!define MUI_ABORTWARNING
!define MUI_FINISHPAGE_RUN "$INSTDIR\${EXE}"
!define MUI_FINISHPAGE_RUN_TEXT "Launch ${APP}"

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_LICENSE "LICENSE.txt"
!insertmacro MUI_PAGE_COMPONENTS
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "Turkish"
!insertmacro MUI_LANGUAGE "English"

VIProductVersion "${VERSION}.0"
VIAddVersionKey /LANG=${LANG_ENGLISH} "ProductName" "${APP}"
VIAddVersionKey /LANG=${LANG_ENGLISH} "FileDescription" "${APP} Setup"
VIAddVersionKey /LANG=${LANG_ENGLISH} "FileVersion" "${VERSION}"
VIAddVersionKey /LANG=${LANG_ENGLISH} "LegalCopyright" "Apache-2.0"

Section "${APP} (required)" SecMain
  SectionIn RO
  SetOutPath "$INSTDIR"
  File "${SRC}\${EXE}"
  File "${SRC}\forge-cli.exe"
  File "forge.ico"
  File "LICENSE.txt"
  SetOutPath "$INSTDIR\fonts"
  File /r "fonts\*.*"
  SetOutPath "$INSTDIR"
  WriteRegStr HKLM "Software\ForgeDesign" "InstallDir" "$INSTDIR"
  WriteUninstaller "$INSTDIR\Uninstall.exe"
  WriteRegStr HKLM "${REGKEY}" "DisplayName" "${APP}"
  WriteRegStr HKLM "${REGKEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKLM "${REGKEY}" "Publisher" "burkiuze"
  WriteRegStr HKLM "${REGKEY}" "DisplayIcon" "$INSTDIR\forge.ico"
  WriteRegStr HKLM "${REGKEY}" "UninstallString" '"$INSTDIR\Uninstall.exe"'
  WriteRegDWORD HKLM "${REGKEY}" "NoModify" 1
  WriteRegDWORD HKLM "${REGKEY}" "NoRepair" 1
  ${GetSize} "$INSTDIR" "/S=0K" $0 $1 $2
  WriteRegDWORD HKLM "${REGKEY}" "EstimatedSize" $0
  CreateDirectory "$SMPROGRAMS\${APP}"
  CreateShortcut "$SMPROGRAMS\${APP}\${APP}.lnk" "$INSTDIR\${EXE}" "" "$INSTDIR\forge.ico"
  CreateShortcut "$SMPROGRAMS\${APP}\Uninstall.lnk" "$INSTDIR\Uninstall.exe"
SectionEnd

Section "Desktop shortcut" SecDesktop
  CreateShortcut "$DESKTOP\${APP}.lnk" "$INSTDIR\${EXE}" "" "$INSTDIR\forge.ico"
SectionEnd

Section "Open .ora / .psd files with Forge Design" SecAssoc
  WriteRegStr HKCR "ForgeDesign.Document" "" "Forge Design Document"
  WriteRegStr HKCR "ForgeDesign.Document\DefaultIcon" "" "$INSTDIR\forge.ico"
  WriteRegStr HKCR "ForgeDesign.Document\shell\open\command" "" '"$INSTDIR\${EXE}" "%1"'
  WriteRegStr HKCR ".ora" "" "ForgeDesign.Document"
  WriteRegStr HKCR ".psd\OpenWithProgids" "ForgeDesign.Document" ""
  WriteRegStr HKCR ".png\OpenWithProgids" "ForgeDesign.Document" ""
  WriteRegStr HKCR ".jpg\OpenWithProgids" "ForgeDesign.Document" ""
  System::Call 'shell32::SHChangeNotify(i 0x08000000, i 0, p 0, p 0)'
SectionEnd

LangString DESC_Main ${LANG_TURKISH} "Forge Design uygulaması ve komut satırı aracı (forge-cli)."
LangString DESC_Main ${LANG_ENGLISH} "The Forge Design application and the forge-cli batch tool."
LangString DESC_Desktop ${LANG_TURKISH} "Masaüstüne kısayol ekler."
LangString DESC_Desktop ${LANG_ENGLISH} "Adds a desktop shortcut."
LangString DESC_Assoc ${LANG_TURKISH} ".ora dosyalarını Forge Design ile açar; .psd/.png/.jpg için 'Birlikte aç' seçeneği ekler."
LangString DESC_Assoc ${LANG_ENGLISH} "Opens .ora files with Forge Design and adds it to 'Open with' for .psd/.png/.jpg."
!insertmacro MUI_FUNCTION_DESCRIPTION_BEGIN
  !insertmacro MUI_DESCRIPTION_TEXT ${SecMain} $(DESC_Main)
  !insertmacro MUI_DESCRIPTION_TEXT ${SecDesktop} $(DESC_Desktop)
  !insertmacro MUI_DESCRIPTION_TEXT ${SecAssoc} $(DESC_Assoc)
!insertmacro MUI_FUNCTION_DESCRIPTION_END

Section "Uninstall"
  Delete "$INSTDIR\${EXE}"
  Delete "$INSTDIR\forge-cli.exe"
  Delete "$INSTDIR\forge.ico"
  Delete "$INSTDIR\LICENSE.txt"
  Delete "$INSTDIR\Uninstall.exe"
  RMDir /r "$INSTDIR\fonts"
  RMDir "$INSTDIR"
  Delete "$DESKTOP\${APP}.lnk"
  RMDir /r "$SMPROGRAMS\${APP}"
  DeleteRegKey HKLM "${REGKEY}"
  DeleteRegKey HKLM "Software\ForgeDesign"
  DeleteRegKey HKCR "ForgeDesign.Document"
  ReadRegStr $0 HKCR ".ora" ""
  StrCmp $0 "ForgeDesign.Document" 0 +2
    DeleteRegKey HKCR ".ora"
  DeleteRegValue HKCR ".psd\OpenWithProgids" "ForgeDesign.Document"
  DeleteRegValue HKCR ".png\OpenWithProgids" "ForgeDesign.Document"
  DeleteRegValue HKCR ".jpg\OpenWithProgids" "ForgeDesign.Document"
SectionEnd
