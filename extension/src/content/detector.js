/**
 * AI 平台检测（ChatGPT / Claude / Gemini）
 */

export const ADAPTERS = {
  chatgpt: {
    match: () => location.hostname === 'chatgpt.com',
    inputBox: '#prompt-textarea',
    sendButton: 'button[data-testid="send-button"]',
    messages: '[data-message-author-role]',
  },
  claude: {
    match: () => location.hostname === 'claude.ai',
    inputBox: '[contenteditable="true"].ProseMirror',
    sendButton: 'button[aria-label="Send Message"]',
    messages: '[data-testid="conversation-turn"]',
  },
  gemini: {
    match: () => location.hostname === 'gemini.google.com',
    inputBox: '.ql-editor',
    sendButton: 'button[aria-label="Send message"]',
    messages: '.conversation-container',
  },
};

export function detectPlatform() {
  for (const [name, adapter] of Object.entries(ADAPTERS)) {
    if (adapter.match()) return { name, ...adapter };
  }
  return null;
}
