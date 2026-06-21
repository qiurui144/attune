/** useMember — 会员状态 + SettingsLocks 拉取与登出.
 *  对接 /api/v1/member/{state,locks,logout}.
 */
import { api } from '../store/api';
import { t } from '../i18n';
import {
  memberState,
  memberVertical,
  settingsLocks,
  type MemberStateKind,
  type SettingsLocksMap,
} from '../store/signals';

/** 会员场景 (vertical) → 本地化场景名 i18n key.
 *  GAP-B (spec 2026-06-20): vertical 是 cloud 下发的会员场景属性,**纯展示文案**,
 *  不参与任何插件门禁(装什么由签名快照 allowed_plugins 决定)。未知 vertical 走
 *  兜底原样显示 (前向兼容未来 cloud 新增 vertical)。 */
const VERTICAL_LABEL_KEYS: Record<string, string> = {
  law: 'member.vertical.law',
  medical: 'member.vertical.medical',
  patent: 'member.vertical.patent',
  presales: 'member.vertical.presales',
  tech: 'member.vertical.tech',
  academic: 'member.vertical.academic',
};

/** vertical id → 本地化场景名;未知值原样返回 (兜底,不 panic / 不丢信息)。 */
export function verticalLabel(vertical: string | null | undefined): string {
  if (!vertical) return '';
  const key = VERTICAL_LABEL_KEYS[vertical];
  return key ? t(key) : vertical;
}

/** 会员入口 (login / activate) 的透传响应形态 (vertical + plugin_sync)。 */
type MemberPluginSync = {
  installed?: string[];
  skipped_already_installed?: string[];
  failed?: { plugin_id: string; reason: string }[];
};
type MemberLoginResp = {
  vertical?: string | null;
  plugin_sync?: MemberPluginSync | null;
};

/** 构建"已为〔{场景}〕场景安装 {plugins}"提示文案.
 *  vertical 缺省 (未指定场景的 pro 用户) 或无新装插件 → 返回 null (不弹该提示)。 */
export function verticalInstallMessage(resp: MemberLoginResp | null | undefined): string | null {
  if (!resp) return null;
  const label = verticalLabel(resp.vertical);
  if (!label) return null;
  const installed = resp.plugin_sync?.installed ?? [];
  if (installed.length === 0) return null;
  return t('member.vertical.installed_for', {
    vertical: label,
    plugins: installed.join('、'),
  });
}

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
): Promise<{ ok: boolean; error?: string; verticalMessage?: string | null }> {
  try {
    // SECURITY: the accounts URL is resolved server-side from settings
    // (settings.cloud.accounts_url) — never sent from the client (SSRF/paywall).
    // Self-host operators configure it once under Settings → cloud.
    const resp = await api.post<MemberLoginResp>('/member/login-password', { email, password });
    await loadMemberState();
    await loadSettingsLocks();
    // GAP-B: remember the cloud-issued vertical for display (Marketplace "current
    // scene" + login toast). vertical is UI copy only (never a gate); the client
    // only relays the cloud value, it never self-reports one.
    memberVertical.value = resp.vertical ?? null;
    return { ok: true, verticalMessage: verticalInstallMessage(resp) };
  } catch (e: unknown) {
    const msg = e instanceof Error ? e.message : String(e);
    return { ok: false, error: msg };
  }
}

export async function memberLogout(): Promise<boolean> {
  try {
    await api.post('/member/logout', {});
    memberState.value = null;
    memberVertical.value = null;
    await loadSettingsLocks();
    return true;
  } catch {
    return false;
  }
}
