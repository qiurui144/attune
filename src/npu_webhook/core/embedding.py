"""Embedding 引擎抽象（ONNX/OpenVINO）"""

from abc import ABC, abstractmethod


class EmbeddingEngine(ABC):
    """Embedding 引擎基类"""

    @abstractmethod
    def embed(self, texts: list[str]) -> list[list[float]]:
        """将文本列表转换为向量列表"""
        ...

    @abstractmethod
    def get_dimension(self) -> int:
        """返回向量维度"""
        ...


class ONNXEmbedding(EmbeddingEngine):
    """ONNX Runtime Embedding（CPU/DirectML/ROCm）"""

    def __init__(self, model_path: str, device: str = "cpu") -> None:
        self.model_path = model_path
        self.device = device
        # TODO Phase 1: 初始化 ONNX Runtime session

    def embed(self, texts: list[str]) -> list[list[float]]:
        # TODO Phase 1
        raise NotImplementedError

    def get_dimension(self) -> int:
        # TODO Phase 1
        return 512


class OpenVINOEmbedding(EmbeddingEngine):
    """OpenVINO Embedding（Intel NPU/iGPU/CPU）"""

    def __init__(self, model_path: str, device: str = "NPU") -> None:
        self.model_path = model_path
        self.device = device
        # TODO Phase 4: 初始化 OpenVINO

    def embed(self, texts: list[str]) -> list[list[float]]:
        # TODO Phase 4
        raise NotImplementedError

    def get_dimension(self) -> int:
        # TODO Phase 4
        return 512


class EmbeddingFactory:
    """根据硬件检测创建最优 Embedding 引擎"""

    @staticmethod
    def create(model_path: str, preference: str = "auto") -> EmbeddingEngine:
        # TODO Phase 1: 硬件检测 + 引擎选择
        return ONNXEmbedding(model_path, device="cpu")
