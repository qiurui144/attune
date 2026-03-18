"""Embedding 引擎测试"""


def test_embedding_factory_creates_cpu_fallback():
    """默认创建 CPU ONNX 引擎"""
    from npu_webhook.core.embedding import EmbeddingFactory, ONNXEmbedding

    engine = EmbeddingFactory.create(model_path="dummy")
    assert isinstance(engine, ONNXEmbedding)
    assert engine.device == "cpu"
