"""搜索引擎测试"""


def test_hybrid_search_engine_init():
    from npu_webhook.core.search import HybridSearchEngine

    engine = HybridSearchEngine()
    assert engine is not None
