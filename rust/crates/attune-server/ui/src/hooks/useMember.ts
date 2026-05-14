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
  // 后端实际扁平化: { kind: 'paid' | 'free', account_id, license_id? }
  // (旧 Rust enum tag 形式 { Paid: {...} } 也兼容)
  if (typeof raw.state === 'object') {
    const s = raw.state as Record<string, unknown>;
    if (typeof s.kind === 'string') {
      if (s.kind === 'paid') return 'paid';
      if (s.kind === 'free') return 'free';
      if (s.kind === 'logged_out') return 'logged_out';
    }
    if ('Free' in s) return 'free';
    if ('Paid' in s) return 'paid';
  }
  // top-level is_paid 兜底 (后端给了 boolean)
  if (raw.is_paid) return 'paid';
  if (raw.is_logged_in) return 'free';
  return 'logged_out';
}

function parseLicenseId(raw: RawState): string | null {
  if (typeof raw.state === 'object') {
    const s = raw.state as Record<string, unknown>;
    if (typeof s.license_id === 'string') return s.license_id;
    if ('Paid' in s) {
      const paid = (s as { Paid?: { license_id?: string } }).Paid;
      if (paid?.license_id) return paid.license_id;
    }
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
    // 后端 SettingsLocks serde 实际输出小写 "editable" / "locked" (新版本)
    // 兼容旧大写 "Editable" / "Locked".
    const raw = await api.get<Record<string, string>>('/member/locks');
    const norm = (v?: string): 'editable' | 'locked' => {
      const lower = (v ?? '').toLowerCase();
      return lower === 'locked' ? 'locked' : 'editable';
    };
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
