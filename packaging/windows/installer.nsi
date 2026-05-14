; NSIS 安装器脚本
!include "MUI2.nsh"

Name "Attune"
OutFile "attune-setup-x64.exe"
InstallDir "$LOCALAPPDATA\Attune"
RequestExecutionLevel user

!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_LANGUAGE "SimpChinese"

Section "Install"
  SetOutPath "$INSTDIR"
  File /r "dist\attune-python\*.*"
  CreateShortcut "$DESKTOP\Attune.lnk" "$INSTDIR\attune-python.exe"
  ; TODO Phase 5: 注册服务 + 开机自启
SectionEnd

Section "Uninstall"
  RMDir /r "$INSTDIR"
  Delete "$DESKTOP\Attune.lnk"
SectionEnd
