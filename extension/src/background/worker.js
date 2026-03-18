/**
 * Background Service Worker - 消息路由 + API 调度
 */

// TODO Phase 2: 消息路由、API 调用、去重逻辑

chrome.runtime.onInstalled.addListener(() => {
  console.log('npu-webhook extension installed');
});
