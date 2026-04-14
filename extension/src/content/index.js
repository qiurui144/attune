/**
 * Content Script 入口 — 整合 capture + injector + indicator
 *
 * 修复：
 * - 消息监听器去重，防止重复注入后重复绑定
 * - 各初始化步骤独立容错，单步失败不阻断其他功能
 */

import { detectPlatform } from './detector.js';
import { ConversationCapture } from './capture.js';
// 2026-04-12: injector 已弃用，产品方向转为内置 Chat + RAG
// 不再向 AI 网站 DOM 注入内容
// import { PrefixInjector } from './injector.js';
import { StatusIndicator } from './indicator.js';
import { MSG, sendToWorker } from '../shared/messages.js';

const platform = detectPlatform();
if (platform) {
  console.log(`[npu-webhook] Detected platform: ${platform.name}`);

  const indicator = new StatusIndicator();
  const capture = new ConversationCapture(platform);

  // 防止 content script 被重复注入时重复绑定
  let _messageListenerAttached = false;

  async function init() {
    indicator.mount();
    indicator.setState('processing');

    // 检查后端连接（失败不阻断 capture 启动）
    try {
      const status = await sendToWorker(MSG.GET_STATUS);
      indicator.setState(status?.online ? 'captured' : 'offline');
    } catch {
      indicator.setState('offline');
    }

    // 启动捕获（被动收集，不再注入）
    try {
      capture.start();
    } catch (err) {
      console.error('[npu-webhook] Capture start failed:', err);
    }

    // 绑定消息监听器（去重防止重复绑定）
    if (!_messageListenerAttached) {
      _messageListenerAttached = true;
      chrome.runtime.onMessage.addListener((msg) => {
        if (msg.type === MSG.SETTINGS_UPDATED) {
          sendToWorker(MSG.GET_STATUS)
            .then((status) => indicator.setState(status?.online ? 'captured' : 'offline'))
            .catch(() => indicator.setState('offline'));
        }
      });
    }
  }

  init();
}
