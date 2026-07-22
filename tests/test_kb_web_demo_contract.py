from pathlib import Path
import re


ROOT = Path(__file__).resolve().parents[1]
DEMO = ROOT / "kb-web-demo"


def html() -> str:
    return (DEMO / "index.html").read_text(encoding="utf-8")


def test_demo_exposes_required_attune_showcase_views():
    page = html()
    for label in ("上传 & 管理", "向量库", "Chat RAG", "Summary RAG"):
        assert label in page


def test_demo_business_fetches_use_attune_api_v1_only():
    page = html()
    paths = re.findall(r"""['"](/[^'"]+)['"]""", page)
    business_paths = [
        p for p in paths
        if not p.startswith("/api/v1/") and not p.startswith("/tmp/")
    ]
    assert business_paths == []


def test_demo_polls_async_chat_jobs_through_attune_proxy():
    page = html()
    assert "/api/v1/chat/local-scheduler/jobs/" in page
    assert "/scheduler/jobs/" not in page


def test_model_change_does_not_call_itself():
    page = html()
    match = re.search(r"async function prepareModel\([^)]*\)\{(?P<body>.*?)\n\}", page, re.S)
    assert match is not None
    assert "prepareModel(" not in match.group("body")


def test_k3_launcher_references_repo_proxy_file():
    script = (DEMO / "start_k3.sh").read_text(encoding="utf-8")
    assert "cors-proxy.py" in script
    assert "cors-proxy2.py" not in script
    assert 'pgrep -f "attune-server.*18906"' not in script


def test_upload_ready_requires_actual_search_hit():
    page = html()
    assert "info && info.chunks>0 && i>1" not in page
    assert "chunks.length" in page
    assert "status='ready'" in page


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
