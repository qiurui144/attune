/**
 * chrome.storage 封装
 */

export async function getSettings() {
  const result = await chrome.storage.local.get('settings');
  return result.settings || {
    backendUrl: 'http://localhost:18900',
    injectionMode: 'auto', // auto/manual/disabled
    excludedDomains: [],
  };
}

export async function saveSettings(settings) {
  await chrome.storage.local.set({ settings });
}
