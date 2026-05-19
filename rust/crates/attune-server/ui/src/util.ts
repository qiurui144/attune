/** 杂项前端工具。 */

/**
 * 生成唯一 ID。
 *
 * 优先用 `crypto.randomUUID`（标准 UUIDv4），但该 API 仅在**安全上下文**
 * （HTTPS / localhost / 127.0.0.1）可用 —— 经裸 HTTP + 局域网 IP 访问 attune
 * 时它不存在，直接调用会抛 `crypto.randomUUID is not a function` 导致整个
 * 前端启动失败。此处缺失时降级到非加密随机串。
 *
 * 调用方（toast key、请求关联 ID）只需唯一性，无加密强度需求，降级安全。
 */
export function genId(): string {
  const c = globalThis.crypto as Crypto | undefined;
  if (c && typeof c.randomUUID === 'function') return c.randomUUID();
  return `${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 12)}`;
}
