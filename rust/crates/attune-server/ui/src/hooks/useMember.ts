/** useMember — 会员状态 + SettingsLocks 拉取与登出.
 *  对接 /api/v1/member/{state,locks,logout}.
 */
import { api } from '../store/api';
import {
  memberState,
  settingsLocks,
  type MemberStateKind,
  type SettingsLocksMap,
} from '../store/signals';

type RawState = {
  state:
    | 'logged_out'
    | { Free: { account_id: string } }
    | {
        Paid: {
          account_id: string;
          license_id: string;
          llm_quota_remaining: number;
        };
      };
  is_logged_in: boolean;
  is_paid: boolean;
  account_id: string | null;
};

function parseKind(raw: RawState): MemberStateKind {
  if (raw.state === 'logged_out') return 'logged_out';
  if (typeof raw.state === 'object' && 'Free' in raw.state) return 'free';
  if (typeof raw.state === 'object' && 'Paid' in raw.state) return 'paid';
  return 'logged_out';
}

function parseLicenseId(raw: RawState): string | null {
  if (typeof raw.state === 'object' && 'Paid' in raw.state) {
    return raw.state.Paid.license_id;
  }
  return null;
}

export async function loadMemberState(): Promise<void> {
  try {
    const raw = await api.get<RawState>('/member/state');
    memberState.value = {
      kind: parseKind(raw),
      account_id: raw.account_id,
      license_id: parseLicenseId(raw),
      is_logged_in: raw.is_logged_in,
      is_paid: raw.is_paid,
    };
  } catch {
    memberState.value = null;
  }
}

export async function loadSettingsLocks(): Promise<void> {
  try {
    // 后端 SettingsLocks 是 enum SettingLock — 序列化为字符串 "Editable" / "Locked"
    const raw = await api.get<Record<string, string>>('/member/locks');
    const norm = (v?: string): 'editable' | 'locked' =>
      v === 'Locked' ? 'locked' : 'editable';
    settingsLocks.value = {
      vault_password: norm(raw.vault_password),
      local_folder_links: norm(raw.local_folder_links),
      cloud_llm: norm(raw.cloud_llm),
      plugin_install: norm(raw.plugin_install),
      plugin_uninstall: norm(raw.plugin_uninstall),
      ocr_profiles: norm(raw.ocr_profiles),
    } as SettingsLocksMap;
  } catch {
    settingsLocks.value = null;
  }
}

export async function memberLoginPassword(
  email: string,
  password: string,
  cloudUrl?: string,
): Promise<{ ok: boolean; error?: string }> {
  try {
    await api.post('/member/login-password', { email, password, cloud_url: cloudUrl ?? null });
    await loadMemberState();
    await loadSettingsLocks();
    return { ok: true };
  } catch (e: unknown) {
    const msg = e instanceof Error ? e.message : String(e);
    return { ok: false, error: msg };
  }
}

export async function memberLogout(): Promise<boolean> {
  try {
    await api.post('/member/logout', {});
    memberState.value = null;
    await loadSettingsLocks();
    return true;
  } catch {
    return false;
  }
}
