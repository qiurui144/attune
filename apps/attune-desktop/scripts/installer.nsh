; attune Windows NSIS installer hooks (R-deploy / 2026-05-01)
;
; Tauri 2 NSIS hooks reference: https://tauri.app/v2/distribute/nsis/#hooks
;
; 触发顺序：
;   NSIS_HOOK_PREINSTALL  → dpkg-equivalent 解压前（旧版优雅停）
;   NSIS_HOOK_POSTINSTALL → 解压完成 → 装 Ollama (静默) + 配 GPU
;   NSIS_HOOK_PREUNINSTALL → 卸载前停服务
;   NSIS_HOOK_POSTUNINSTALL → 卸载完清 systemd（Win 用 Service）
;
; Windows 比 Linux 简单：
;   - 不需要 HSA override（CUDA/Vulkan 自动）
;   - Ollama 提供 OllamaSetup.exe 静默模式
;   - 模型拉取依然延后到 attune-desktop 首次启动 wizard

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
  DetailPrint "Checking Ollama installation..."

  ; 检查 ollama 是否在 PATH（where 是 Windows builtin）
  nsExec::ExecToStack 'where ollama'
  Pop $0  ; exit code
  Pop $1  ; output
  ${If} $0 == 0
    DetailPrint "Ollama already installed: $1"
  ${Else}
    DetailPrint "Ollama not found. Downloading installer (~600 MB)..."
    ; 用 inetc plugin (NSIS 标配) 替代过时的 NSISdl
    ; /SILENT 隐藏进度框；/RESUME 失败可恢复；/CAPTION 显示自定义标题
    inetc::get /CAPTION "Downloading Ollama" /POPUP "ollama.com" \
      "https://ollama.com/download/OllamaSetup.exe" "$TEMP\OllamaSetup.exe" /END
    Pop $R0  ; "OK" 或错误描述
    ${If} $R0 == "OK"
      DetailPrint "Running Ollama installer (silent)..."
      ; OllamaSetup.exe 是 NSIS 自身打的包，支持 /S = silent
      ExecWait '"$TEMP\OllamaSetup.exe" /S' $0
      Delete "$TEMP\OllamaSetup.exe"
      ${If} $0 == 0
        DetailPrint "Ollama installed successfully."
      ${Else}
        DetailPrint "WARNING: Ollama installer exited with code $0."
        DetailPrint "  attune-desktop first-run wizard will offer manual install instructions."
      ${EndIf}
    ${Else}
      DetailPrint "WARNING: Ollama download failed: $R0"
      DetailPrint "  attune-desktop first-run wizard will display install command for manual run."
    ${EndIf}
  ${EndIf}

  ; Windows 没有 AMD HSA override 这种通用问题（CUDA / DirectML 自动）
  DetailPrint "Ollama service should auto-start (Windows service)."
  DetailPrint "First-run wizard will pull recommended models with progress UI."
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
