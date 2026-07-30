from pathlib import Path
import re


ROOT = Path(__file__).resolve().parents[1]
DEMO = ROOT / "kb-web-demo"
PLAYWRIGHT_GATE = ROOT / "tests" / "e2e" / "playwright" / "kb_web_demo_eval_frontend_e2e.py"


def html() -> str:
    return (DEMO / "index.html").read_text(encoding="utf-8")


def playwright_gate() -> str:
    return PLAYWRIGHT_GATE.read_text(encoding="utf-8")


def test_demo_exposes_required_attune_showcase_views():
    page = html()
    for label in ("上传 & 管理", "向量库", "Chat RAG", "Summary RAG"):
        assert label in page


def test_demo_models_are_loaded_dynamically_from_attune():
    page = html()
    assert "async function refreshModelOptions" in page
    assert "modelsFromAiStack" in page
    assert "'/api/v1/ai-stack'" in page
    assert "data-capability=\"chat\"" in page
    assert "data-capability=\"summary\"" in page
    assert '<option value="llm-chat"' not in page
    assert '<option value="llm-summary"' not in page
    assert "/models" not in page


def test_demo_summary_uses_workflow_endpoint_not_chat_prompt_preset():
    page = html()
    assert "summaryScenario" in page
    assert "summaryDetail" in page
    assert "runSummaryWorkflow" in page
    assert "'/api/v1/summary/workflow'" in page
    assert "summary_sections" in page
    send_match = re.search(r"async function sendRag\([^)]*\)\{(?P<body>.*?)\n\}", page, re.S)
    assert send_match is not None
    assert "总结目标" not in send_match.group("body")


def test_demo_supports_folder_drag_and_folder_picker():
    page = html()
    assert "webkitdirectory" in page
    assert "folderInput" in page
    assert "collectDroppedFiles" in page
    assert "readDirectoryEntries" in page
    assert "uploadOneFile(file, relativePath)" in page
    assert "relative_path" in page


def test_demo_has_attune_only_clear_environment_button():
    page = html()
    assert "清零环境" in page
    assert "async function clearDemoEnvironment" in page
    assert "sessionStorage.removeItem('attune_demo_files')" in page
    assert "'/api/v1/demo/reset'" in page
    assert "CLEAR_DEMO" in page


def test_demo_business_fetches_use_attune_api_v1_only():
    page = html()
    paths = re.findall(r"""['"](/[^'"]+)['"]""", page)
    business_paths = [
        p for p in paths
        if not p.startswith("/api/v1/") and not p.startswith("/tmp/")
    ]
    assert business_paths == []


def test_demo_supports_bearer_auth_without_url_token():
    page = html()
    script = playwright_gate()
    release_script = (ROOT / "scripts" / "release" / "test-k3-nas-web-demo.sh").read_text(encoding="utf-8")
    assert "function authToken()" in page
    assert "function tokenExpiry(token)" in page
    assert "function authLabel()" in page
    assert "attune_auth_token" in page
    assert "init.headers.Authorization" in page
    assert "async function unlockVault" in page
    assert "'/api/v1/vault/unlock'" in page
    assert "clearAuth()" in page
    assert "localStorage.setItem('attune_auth_token', token)" in page
    assert "authToken() ? `状态异常: ${err.message}` : '需要解锁'" in page
    assert "--token" in script
    assert "page.add_init_script" in script
    assert "page_params[\"token\"]" not in script
    assert "--token \"$TOKEN\"" in release_script


def test_web_demo_eval_preserves_attune_postprocessed_chat_content():
    script = playwright_gate()
    assert "def chat_async_placeholder" in script
    assert "if text and not chat_async_placeholder(text):" in script
    assert "return payload" in script


def test_k3_api_contract_covers_dynamic_models_and_summary_workflow():
    script = (ROOT / "scripts" / "release" / "probe-nas-web-api-contract.py").read_text(encoding="utf-8")
    release_script = (ROOT / "scripts" / "release" / "test-k3-nas-web-demo.sh").read_text(encoding="utf-8")
    assert "model_capability_gate" in script
    assert "summary_workflow_gate" in script
    assert '"/api/v1/summary/workflow"' in script
    assert '"model_capability"' in script
    assert '"summary_workflow"' in script
    assert "dynamic model capability" in release_script
    assert "Summary Workflow Gate" in release_script


def test_project_e2e_entrypoint_is_k3_physical_device_only():
    runner = (ROOT / "tests" / "e2e" / "run_all.sh").read_text(encoding="utf-8")
    release_script = (ROOT / "scripts" / "release" / "test-k3-nas-web-demo.sh").read_text(encoding="utf-8")
    pyramid = (ROOT / "scripts" / "test-pyramid.sh").read_text(encoding="utf-8")
    assert "scripts/release/test-k3-nas-web-demo.sh" in runner
    assert "ATTUNE_K3_HOST" in runner
    assert "is_loopback_or_local" in runner
    assert "cargo build" not in runner
    assert "--no-auth --port" not in runner
    assert "XDG_DATA_HOME" not in runner
    assert "localhost:18905" not in runner
    assert "live K3 E2E requires a physical-device host" in release_script
    assert "live K3 E2E requires a physical-device base URL" in release_script
    assert "ATTUNE_K3_LONGTEXT_E2E=1" in pyramid
    assert "ATTUNE_E2E_LONGTEXT=1" not in pyramid


def test_attune_exposes_voice_receive_api_without_server_audio_packaging():
    server = (ROOT / "rust" / "crates" / "attune-server" / "src" / "lib.rs").read_text(encoding="utf-8")
    probe = (ROOT / "scripts" / "release" / "probe-nas-web-api-contract.py").read_text(encoding="utf-8")
    page = html()
    assert '"/api/v1/voice/status"' in server
    assert '"/api/v1/voice/transcribe"' in server
    assert '"/api/v1/voice/transcribe-file"' in server
    assert '"/api/v1/voice/synthesize"' not in server
    assert "routes::voice::transcribe" in server
    assert "routes::voice::transcribe_file" in server
    assert "routes::office::post_transcribe" in server
    assert "voice_scheduler_gate" in probe
    assert '"/api/v1/voice/status"' in probe
    assert '"/api/v1/voice/transcribe-file"' in probe
    assert '"/api/v1/voice/synthesize"' not in probe
    assert "voiceInput" in page
    assert "accept=\"audio/*\"" in page
    assert "uploadVoiceFile" in page
    assert "'/api/v1/voice/transcribe-file'" in page
    assert "/api/v1/voice/synthesize" not in page
    assert "voice_synthesize_alias" not in (ROOT / "rust" / "crates" / "attune-server" / "tests" / "tts_route_test.rs").read_text(encoding="utf-8")


def test_demo_supports_browser_voice_chat_without_scheduler_audio_access():
    page = html()
    assert "voiceChatRecordBtn" in page
    assert "voiceChatStopBtn" in page
    assert "voiceAutoSpeak" in page
    assert "speakLastBtn" in page
    assert "navigator.mediaDevices.getUserMedia" in page
    assert "new MediaRecorder" in page
    assert "async function submitVoiceChatAudio" in page
    assert "function transcriptFromVoiceJobPayload" in page
    assert "pollVoiceJob(jobId, fileName, t0)" in page
    assert "sendRag('chat', {speak:true})" in page
    assert "async function apiBlob" in page
    assert "'/api/v1/tts/synthesize'" in page
    gate_match = re.search(r"function updateModelActionGate\([^)]*\)\{(?P<body>.*?)\n\}", page, re.S)
    assert gate_match is not None
    assert "voiceChatRecordBtn" in gate_match.group("body")
    assert "locked || !gate.ready || !!voiceRecorder" in gate_match.group("body")
    assert "/api/v1/voice/synthesize" not in page
    assert "fetch(SCHEDULER" not in page


def test_demo_supports_webrtc_voice_receive_loopback_attune_call_path():
    page = html()
    assert "webrtcVoiceTestBtn" in page
    assert "async function attachVoiceChatRemoteStream" in page
    assert "async function recordVoiceChatStream" in page
    assert "async function runVoiceChatWebRtcLoopbackTest" in page
    assert "new RTCPeerConnection" in page
    assert ".ontrack" in page
    assert "pcSender.addTrack" in page
    assert "pcReceiver.setRemoteDescription" in page
    assert "pcSender.setRemoteDescription" in page
    assert "submitVoiceChatAudio(blob, mimeType)" in page
    assert "createSyntheticVoiceStream" in page
    assert "'/api/v1/voice/transcribe-file'" in page
    assert "'/api/v1/chat'" in page
    assert "'/api/v1/tts/synthesize'" in page
    assert "/api/v1/voice/synthesize" not in page
    assert "fetch(SCHEDULER" not in page


def test_demo_default_api_base_follows_current_page_port():
    page = html()
    assert "function defaultApiBase()" in page
    assert "const apiPort = port > 0 ? port + 1 : 8969" in page
    assert "defaultApiBase()" in page
    assert "localStorage.getItem('attune_api_base')" not in page
    assert "192.168.100.233:8889" not in page


def test_demo_polls_async_chat_jobs_through_attune_proxy():
    page = html()
    assert "/api/v1/chat/local-scheduler/jobs/" in page
    assert "/scheduler/jobs/" not in page


def test_model_change_does_not_call_itself():
    page = html()
    match = re.search(r"async function prepareModel\([^)]*\)\{(?P<body>.*?)\n\}", page, re.S)
    assert match is not None
    assert "prepareModel(" not in match.group("body")


def test_model_change_closes_wait_state_when_model_not_ready():
    page = html()
    assert "function modelReadinessFromAiStack" in page
    assert "async function ensureSelectedModelReady" in page
    assert "'/api/v1/ai-stack'" in page
    assert "function closeModelWait" in page
    assert "模型未就绪" in page
    match = re.search(r"async function prepareModel\([^)]*\)\{(?P<body>.*?)\n\}", page, re.S)
    assert match is not None
    body = match.group("body")
    assert "try{" in body
    assert "catch(err)" in body
    assert "finally{" in body
    assert "btn.disabled = false" in body
    assert "ensureSelectedModelReady(model, capability)" in body


def test_model_switch_blocks_matching_chat_or_summary_actions():
    page = html()
    assert "const modelSwitchState" in page
    assert "function setModelInteractionLocked" in page
    assert "function updateModelActionGate" in page
    assert "function modelActionAllowed" in page
    assert "function assertModelActionAllowed" in page
    assert "model-switching" in page
    assert "模型切换中" in page
    assert "模型未就绪" in page
    assert 'data-ready="${row.ready ? \'1\' : \'0\'}"' in page
    assert "capabilities: caps.length ? caps : (row.source === 'attune-settings' ? ['chat','summary'] : [])" in page
    prepare_match = re.search(r"async function prepareModel\([^)]*\)\{(?P<body>.*?)\n\}", page, re.S)
    assert prepare_match is not None
    prepare_body = prepare_match.group("body")
    assert "const capability = kind === 'summary' ? 'summary' : 'chat'" in prepare_body
    assert "ensureSelectedModelReady(model, capability)" in prepare_body
    assert "setModelInteractionLocked(kind, true)" in prepare_body
    assert "setModelInteractionLocked(kind, false)" in prepare_body
    assert "updateModelActionGate(kind)" in prepare_body
    send_match = re.search(r"async function sendRag\([^)]*\)\{(?P<body>.*?)\n\}", page, re.S)
    assert send_match is not None
    assert "assertModelActionAllowed('chat')" in send_match.group("body")
    summary_match = re.search(r"async function runSummaryWorkflow\([^)]*\)\{(?P<body>.*?)\n\}", page, re.S)
    assert summary_match is not None
    assert "assertModelActionAllowed('summary')" in summary_match.group("body")


def test_k3_launcher_references_repo_proxy_file():
    script = (DEMO / "start_k3.sh").read_text(encoding="utf-8")
    assert "cors-proxy.py" in script
    assert "cors-proxy2.py" not in script
    assert 'pgrep -f "attune-server.*18906"' not in script


def test_k3_launcher_allows_overriding_scheduler_and_server_binary():
    script = (DEMO / "start_k3.sh").read_text(encoding="utf-8")
    assert "ATTUNE_SCHEDULER_URL" in script
    assert "ATTUNE_SERVER_BIN" in script
    assert "/usr/bin/attune-server-headless --no-auth" not in script
    assert '\\"endpoint\\":\\"${SCHEDULER_URL}\\"' in script
    assert "&scheduler=" not in script


def test_demo_never_exposes_or_sets_scheduler_endpoint():
    page = html()
    assert "SCHEDULER_BASE" not in page
    assert "params.get('scheduler')" not in page
    assert "attune_scheduler_base" not in page
    assert "endpoint:SCHEDULER_BASE" not in page
    assert "endpoint:'http://127.0.0.1:8090'" not in page
    prepare_match = re.search(r"async function prepareModel\([^)]*\)\{(?P<body>.*?)\n\}", page, re.S)
    assert prepare_match is not None
    assert "endpoint" not in prepare_match.group("body")
    assert "fetch(SCHEDULER" not in page
    assert "fetch('http" not in page
    assert "fetch(\"http" not in page


def test_playwright_gate_does_not_pass_scheduler_url_to_demo_page():
    script = playwright_gate()
    assert "--scheduler-url" not in script
    assert "args.scheduler_url" not in script
    assert 'page_params["scheduler"]' not in script
    assert "urllib.parse.urlencode" in script


def test_upload_ready_uses_item_stats_before_search_probe():
    page = html()
    wait_match = re.search(r"async function waitUntilSearchable\([^)]*\)\{(?P<body>.*?)\n\}", page, re.S)
    assert wait_match is not None
    body = wait_match.group("body")
    assert "`/api/v1/items/${encodeURIComponent(itemId)}/stats`" in body
    assert "embedding_pending" in body
    assert "embedding_done" in body
    assert "if(pending === 0){" in body
    assert "if(chunks.length || done > 0 || total > 0 || (info && info.chunks > 0)){" in body
    assert "status='ready'" in page


def test_upload_ready_preserves_queued_chunk_count():
    page = html()
    wait_match = re.search(r"async function waitUntilSearchable\([^)]*\)\{(?P<body>.*?)\n\}", page, re.S)
    assert wait_match is not None
    body = wait_match.group("body")
    assert "timings.chunks = chunks.length" not in body
    assert "info.chunks=timings.chunks" not in body
    assert "search_hits" in page
    assert "hits" in page


def test_rag_chat_blocks_until_index_is_ready():
    page = html()
    assert "ensureKnowledgeReadyForRag" in page
    send_match = re.search(r"async function sendRag\([^)]*\)\{(?P<body>.*?)\n\}", page, re.S)
    assert send_match is not None
    assert "ensureKnowledgeReadyForRag()" in send_match.group("body")


def test_rag_meta_prefers_chat_api_latency_and_model_fields():
    page = html()
    assert "function latencyFromApiPayload" in page
    assert "function modelFromApiPayload" in page
    assert "function providerFromApiPayload" in page
    send_match = re.search(r"async function sendRag\([^)]*\)\{(?P<body>.*?)\n\}", page, re.S)
    assert send_match is not None
    body = send_match.group("body")
    assert "latencyFromApiPayload(d, elapsed)" in body
    assert "modelFromApiPayload(d, model)" in body
    assert "providerFromApiPayload(d)" in body
    assert "fmtMs(totalElapsed)" not in body
