"""Embedding 引擎测试"""


def test_create_embedding_engine_no_model():
    """模型不存在时返回 None"""
    from pathlib import Path
    from npu_webhook.core.embedding import create_embedding_engine

    engine = create_embedding_engine(
        model_name="nonexistent-model",
        device="cpu",
        data_dir=Path("/tmp/npu-webhook-test-nonexistent"),
    )
    assert engine is None
