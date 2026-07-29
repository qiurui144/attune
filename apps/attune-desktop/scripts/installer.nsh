; attune Windows NSIS installer hooks
;
; Tauri 2 NSIS hooks reference: https://tauri.app/v2/distribute/nsis/#hooks
;
; 设计原则: installer 只负责 attune 本体, 不下载外部依赖.
; Cloud/BYOK 或 edge scheduler endpoint 配置全部延后到首启 wizard, 原因:
;   - install-time download 受 corporate firewall 影响易 fail
;   - GitHub Actions Windows runner 默认 NSIS 不带 inetc plugin
;     (历史 desktop-v0.6.3-rc.1 build fail: "Plugin not found, cannot call inetc::get")
;   - scheduler/cloud 边界应由用户或设备镜像显式配置, installer 不碰具体 worker

!macro NSIS_HOOK_PREINSTALL
  ; 停旧版 attune 进程（升级时）
  DetailPrint "Stopping any running attune processes..."
  nsExec::ExecToStack 'taskkill /F /IM attune-server-headless.exe /T'
  Pop $0
  nsExec::ExecToStack 'taskkill /F /IM attune-desktop.exe /T'
  Pop $0
  Sleep 1000
!macroend

!macro NSIS_HOOK_POSTINSTALL
  ; SenseVoice ASR native libs (sherpa-onnx + bundled onnxruntime) must sit BESIDE the
  ; main exe so the Windows loader (which searches the exe's own directory) resolves the
  ; hard-linked sherpa DLLs at launch. Tauri bundles them (via tauri.conf `resources`)
  ; into $INSTDIR\lib\windows\ ; copy them next to the exe ($INSTDIR). If the build had no
  ; asr-sensevoice feature, the dir is absent and we skip — ASR degrades to whisper-cli.
  ; PENDING-rc.5-CI: real Windows package load smoke.
  ${If} ${FileExists} "$INSTDIR\lib\windows\*.dll"
    DetailPrint "Installing SenseVoice ASR runtime libraries..."
    CopyFiles /SILENT "$INSTDIR\lib\windows\*.dll" "$INSTDIR"
  ${EndIf}

  ; Enterprise deployment tools need a non-interactive uninstall command for
  ; rollback / fleet removal. Tauri registers UninstallString by default; add the
  ; quiet variant explicitly for winget/SCCM/GPO style automation.
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Attune" \
    "QuietUninstallString" '"$INSTDIR\uninstall.exe" /S'

  DetailPrint "attune installation complete."
  DetailPrint "First-run wizard will configure cloud/BYOK or an edge scheduler endpoint."
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  DetailPrint "Stopping attune processes before uninstall..."
  nsExec::ExecToStack 'taskkill /F /IM attune-server-headless.exe /T'
  Pop $0
  nsExec::ExecToStack 'taskkill /F /IM attune-desktop.exe /T'
  Pop $0
  Delete "$APPDATA\Microsoft\Windows\Start Menu\Programs\Startup\Attune.lnk"
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ; 不卸载第三方 AI runtime — 用户可能其他场景在用
  DetailPrint "Note: third-party AI runtimes and model files are preserved."
!macroend
