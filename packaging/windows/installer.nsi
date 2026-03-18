; NSIS 安装器脚本
!include "MUI2.nsh"

Name "npu-webhook"
OutFile "npu-webhook-setup-x64.exe"
InstallDir "$LOCALAPPDATA\npu-webhook"
RequestExecutionLevel user

!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_LANGUAGE "SimpChinese"

Section "Install"
  SetOutPath "$INSTDIR"
  File /r "dist\npu-webhook\*.*"
  CreateShortcut "$DESKTOP\npu-webhook.lnk" "$INSTDIR\npu-webhook.exe"
  ; TODO Phase 5: 注册服务 + 开机自启
SectionEnd

Section "Uninstall"
  RMDir /r "$INSTDIR"
  Delete "$DESKTOP\npu-webhook.lnk"
SectionEnd
