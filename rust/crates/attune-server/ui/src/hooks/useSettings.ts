/** useSettings · 读写 app_settings */
import { api } from '../store/api';
import { settings } from '../store/signals';

export type AppSettings = Record<string, unknown>;

export async function loadSettings(): Promise<AppSettings> {
  try {
    const s = await api.get<AppSettings>('/settings');
    settings.value = s;
    return s;
  } catch {
    return {};
  }
}

export async function patchSettings(patch: AppSettings): Promise<boolean> {
  try {
    const merged = await api.patch<AppSettings>('/settings', patch);
    settings.value = merged;
    return true;
  } catch {
    return false;
  }
}
