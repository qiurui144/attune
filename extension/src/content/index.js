/**
 * Content Script 入口
 */

import { detectPlatform } from './detector.js';

const platform = detectPlatform();
if (platform) {
  console.log(`[npu-webhook] Detected platform: ${platform.name}`);
  // TODO Phase 2: 初始化 capture + injector + indicator
}
