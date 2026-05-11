"""Embedding 引擎：ONNX Runtime / Ollama 加载 embedding 模型"""

import logging
from abc import ABC, abstractmethod
from pathlib import Path

import numpy as np

logger = logging.getLogger(__name__)


class EmbeddingEngine(ABC):
    """Embedding 引擎基类"""

    @abstractmethod
    def embed(self, texts: list[str]) -> list[list[float]]:
        ...

    @abstractmethod
    def get_dimension(self) -> int:
        ...


class ONNXEmbedding(EmbeddingEngine):
    """ONNX Runtime Embedding（CPU/DirectML/ROCm）"""

    def __init__(self, model_dir: str | Path, device: str = "cpu", max_length: int = 512) -> None:
        import onnxruntime as ort
        from tokenizers import Tokenizer

        self.max_length = max_length
        model_dir = Path(model_dir)

        # 选择 EP
        providers = ["CPUExecutionProvider"]
        if device == "directml":
            providers = ["DmlExecutionProvider", "CPUExecutionProvider"]
        elif device == "rocm":
            providers = ["ROCMExecutionProvider", "CPUExecutionProvider"]

        onnx_path = model_dir / "model.onnx"
        if not onnx_path.exists():
            raise FileNotFoundError(f"ONNX model not found: {onnx_path}")

        self.session = ort.InferenceSession(str(onnx_path), providers=providers)
        self.tokenizer = Tokenizer.from_file(str(model_dir / "tokenizer.json"))
        self.tokenizer.enable_truncation(max_length=self.max_length)
        self.tokenizer.enable_padding(length=self.max_length)

        # 推断维度
        output_shape = self.session.get_outputs()[0].shape
        self._dimension = output_shape[-1] if len(output_shape) > 1 else 512

        logger.info("ONNX embedding loaded: %s (dim=%d, device=%s)", model_dir.name, self._dimension, device)

    def embed(self, texts: list[str]) -> list[list[float]]:
        encodings = self.tokenizer.encode_batch(texts)
        input_ids = np.array([e.ids for e in encodings], dtype=np.int64)
        attention_mask = np.array([e.attention_mask for e in encodings], dtype=np.int64)
        token_type_ids = np.zeros_like(input_ids)

        feeds = {
            "input_ids": input_ids,
            "attention_mask": attention_mask,
            "token_type_ids": token_type_ids,
        }

        # 只保留模型实际需要的输入
        input_names = {inp.name for inp in self.session.get_inputs()}
        feeds = {k: v for k, v in feeds.items() if k in input_names}

        outputs = self.session.run(None, feeds)

        # 取 [CLS] token 的输出作为句向量，然后 L2 归一化
        embeddings = outputs[0][:, 0, :]  # (batch, dim)
        norms = np.linalg.norm(embeddings, axis=1, keepdims=True)
        norms = np.maximum(norms, 1e-12)
        embeddings = embeddings / norms

        return embeddings.tolist()

    def get_dimension(self) -> int:
        return self._dimension


class OllamaEmbedding(EmbeddingEngine):
    """Ollama Embedding（通过 HTTP API 调用本地 Ollama 服务）"""

    def __init__(self, model: str = "bge-m3", base_url: str = "http://localhost:11434") -> None:
        import urllib.request
        import json

        self.model = model
        self.base_url = base_url.rstrip("/")

        # 探测维度：发送一次单文本 embedding
        try:
            data = json.dumps({"model": model, "input": "dim probe"}).encode()
            req = urllib.request.Request(
                f"{self.base_url}/api/embed",
                data=data,
                headers={"Content-Type": "application/json"},
            )
            with urllib.request.urlopen(req, timeout=30) as resp:
                result = json.loads(resp.read())
            self._dimension = len(result["embeddings"][0])
        except OSError as e:
            raise ConnectionError(
                f"Cannot reach Ollama at {self.base_url} — is Ollama running? ({e})"
            ) from e
        except (KeyError, IndexError, ValueError) as e:
            raise ConnectionError(
                f"Ollama responded but model '{model}' may not be pulled: {e}"
            ) from e
        except Exception as e:
            raise ConnectionError(f"Ollama embedding probe failed ({self.base_url}): {e}") from e

        logger.info("Ollama embedding ready: %s (dim=%d, url=%s)", model, self._dimension, self.base_url)

    def embed(self, texts: list[str]) -> list[list[float]]:
        import json
        import urllib.error
        import urllib.request

        data = json.dumps({"model": self.model, "input": texts}).encode()
        req = urllib.request.Request(
            f"{self.base_url}/api/embed",
            data=data,
            headers={"Content-Type": "application/json"},
        )
        try:
            with urllib.request.urlopen(req, timeout=120) as resp:
                result = json.loads(resp.read())
            return result["embeddings"]
        except (urllib.error.URLError, TimeoutError) as e:
            raise ConnectionError(f"Ollama embed request failed: {e}") from e
        except (json.JSONDecodeError, KeyError) as e:
            raise ValueError(f"Ollama embed response invalid: {e}") from e

    def get_dimension(self) -> int:
        return self._dimension


class OpenVINOEmbedding(EmbeddingEngine):
    """OpenVINO Embedding（Intel NPU/iGPU/CPU）- Phase 4 实现"""

    def __init__(self, model_dir: str | Path, device: str = "NPU") -> None:
        self.model_dir = Path(model_dir)
        self.device = device
        raise NotImplementedError("OpenVINO embedding will be implemented in Phase 4")

    def embed(self, texts: list[str]) -> list[list[float]]:
        raise NotImplementedError

    def get_dimension(self) -> int:
        return 512


def _find_model_dir(model_name: str, data_dir: Path) -> Path | None:
    """在多个候选位置查找模型目录"""
    candidates = [
        data_dir / "models" / model_name,
        Path.home() / ".cache" / "npu-webhook" / "models" / model_name,
    ]
    for p in candidates:
        if (p / "model.onnx").exists() and (p / "tokenizer.json").exists():
            return p
    return None


def create_embedding_engine(
    model_name: str = "bge-m3",
    device: str = "auto",
    data_dir: Path | None = None,
    max_length: int = 512,
) -> EmbeddingEngine | None:
    """工厂函数：根据硬件和模型可用性创建 embedding 引擎

    device 取值:
      - ollama: 使用 Ollama HTTP API
      - cpu/directml/rocm: 使用本地 ONNX Runtime
      - openvino: 使用 OpenVINO（Intel NPU/iGPU）
      - auto: 优先 Ollama，回退 ONNX
    """
    # Ollama 模式
    if device in ("ollama", "auto"):
        try:
            return OllamaEmbedding(model=model_name)
        except Exception as e:
            if device == "ollama":
                logger.error("Ollama embedding failed: %s", e)
                return None
            logger.info("Ollama not available, falling back to ONNX: %s", e)

    # OpenVINO 模式（Intel NPU/iGPU）— Phase 4 实现后启用
    if device == "openvino":
        logger.warning("OpenVINO embedding not yet implemented, falling back to ONNX CPU")

    # ONNX 模式
    if data_dir is None:
        from npu_webhook.config import settings
        data_dir = settings.data_dir

    model_dir = _find_model_dir(model_name, data_dir)
    if model_dir is None:
        logger.warning("Model not found: %s. Embedding disabled until model is downloaded.", model_name)
        return None

    actual_device = "cpu"
    if device in ("directml", "rocm"):
        actual_device = device

    return ONNXEmbedding(model_dir, device=actual_device, max_length=max_length)
