"""文件索引测试"""


def test_chunker_init():
    from npu_webhook.core.chunker import Chunker

    chunker = Chunker(chunk_size=256, overlap=64)
    assert chunker.chunk_size == 256
    assert chunker.overlap == 64
