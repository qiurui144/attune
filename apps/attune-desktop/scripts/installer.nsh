; attune Windows NSIS installer hooks
;
; Tauri 2 NSIS hooks reference: https://tauri.app/v2/distribute/nsis/#hooks
;
; 设计原则: installer 只负责 attune 本体, 不下载外部依赖.
; Ollama / 模型拉取全部延后到首启 wizard, 原因:
;   - install-time download 受 corporate firewall 影响易 fail
;   - GitHub Actions Windows runner 默认 NSIS 不带 inetc plugin
;     (历史 desktop-v0.6.3-rc.1 build fail: "Plugin not found, cannot call inetc::get")
;   - 用户在 wizard 看到进度比 installer 静默 hang 600 MB 友好

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
  DetailPrint "attune installation complete."
  DetailPrint "First-run wizard will detect Ollama and guide model setup."
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  DetailPrint "Stopping attune processes before uninstall..."
  nsExec::ExecToStack 'taskkill /F /IM attune-server-headless.exe /T'
  Pop $0
  nsExec::ExecToStack 'taskkill /F /IM attune-desktop.exe /T'
  Pop $0
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ; 不卸载 Ollama — 用户可能其他场景在用
  DetailPrint "Note: Ollama runtime + downloaded models preserved."
  DetailPrint "To remove Ollama: 'OllamaSetup.exe /UNINSTALL' or Settings > Apps > Ollama"
!macroend
