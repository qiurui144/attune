import importlib.util
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
PROBE_PATH = ROOT / "scripts" / "release" / "probe-nas-web-api-contract.py"


def load_probe():
    spec = importlib.util.spec_from_file_location("probe_nas_web_api_contract", PROBE_PATH)
    assert spec is not None
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


def test_core_read_retries_transient_tag_index_startup_error(monkeypatch):
    probe = load_probe()
    attempts = {"count": 0}

    def fake_request_json(*args, **kwargs):
        attempts["count"] += 1
        if attempts["count"] < 3:
            raise probe.ProbeError(
                "GET /api/v1/tags failed HTTP 403: "
                "{'code': 'forbidden', 'error': 'vault locked or tag index unavailable'}"
            )
        return 200, {"dimensions": {}}

    monkeypatch.setattr(probe, "request_json", fake_request_json)
    monkeypatch.setattr(probe.time, "sleep", lambda _seconds: None)

    status, payload = probe.request_core_read_json(
        "http://k3:18900",
        "tags",
        "/api/v1/tags",
        token="token",
        timeout=1.0,
    )

    assert status == 200
    assert payload == {"dimensions": {}}
    assert attempts["count"] == 3


def test_core_read_does_not_retry_non_tag_errors(monkeypatch):
    probe = load_probe()
    attempts = {"count": 0}

    def fake_request_json(*args, **kwargs):
        attempts["count"] += 1
        raise probe.ProbeError("GET /api/v1/items failed HTTP 403: {'error': 'forbidden'}")

    monkeypatch.setattr(probe, "request_json", fake_request_json)

    try:
        probe.request_core_read_json(
            "http://k3:18900",
            "items",
            "/api/v1/items",
            token="token",
            timeout=1.0,
        )
    except probe.ProbeError:
        pass
    else:
        raise AssertionError("non-tag core read errors must not be retried")

    assert attempts["count"] == 1
