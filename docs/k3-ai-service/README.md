# K3 AI 推理服务

四场景 HTTP API，面向 attune/attune-enterprise 产品。

## 服务地址

`http://192.168.100.209:8080`

| 接口 | 延迟 (P50) | 模型 |
|:-----|:----------|:-----|
| `POST /v1/embeddings` | bge-small **75ms**, bge-base **505ms** | 768d/512d 向量 |
| `POST /v1/rerank` | **1032ms** | bge-reranker-base |
| `POST /v1/transcribe` | **5550ms** | whisper-small Q8_0 IME |
| `POST /v1/ocr` | **12500ms** | PPOCRv5 det+rec |
| `GET /health` | <1ms | 健康检查 |

20 轮稳定性测试零失败，systemd 开机自启。

## 管理

```bash
systemctl start k3-ai     # 启动
systemctl stop k3-ai      # 停止
systemctl status k3-ai    # 状态
bash start.sh rvv         # 切换到 RVV 上游模式
bash start.sh ime         # 切换到 IME 商业模式
```

## 文档

- [部署文档](docs/K3_AI_SERVICE_DEPLOY.md) — API 文档、对接示例、性能基准
- [开发文档](docs/K3_AI_SERVICE_DEVELOP.md) — 架构、构建、双线策略

## 双线策略

- **IME 商业线**：SpacemiT vmadot 私有指令，INT8 比 RVV 快 30-49%
- **RVV 上游线**：纯标准 RVV，可提交 ORT/llama.cpp upstream PR
- FP32 dispatch 两线一致（27ms/144ms/143ms）
