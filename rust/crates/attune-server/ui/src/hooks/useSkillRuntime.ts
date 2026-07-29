/**
 * Skills orchestration-runtime client — list / estimate / run a declarative skill.
 *
 * A skill run is 💰 (multi-step LLM, user-triggered): the cost contract requires we estimate
 * BEFORE running and pass `confirm_cost: true` on run. The run returns the downloadable artifact
 * inline (base64) — we decode it and trigger a browser download (no server-side artifact store).
 */

import { getToken } from '../store/api';

function authHeaders(): Record<string, string> {
  const token = getToken();
  const headers: Record<string, string> = { 'Content-Type': 'application/json' };
  if (token) headers.Authorization = `Bearer ${token}`;
  return headers;
}

export interface SkillInputInfo {
  name: string;
  ty: string; // 'itemid' | 'string' | 'stringlist'
  required: boolean;
}

export interface SkillInfo {
  id: string;
  version: string;
  current_version?: string;
  title: string;
  description: string;
  cost_tier: string; // 'free' | 'local' | 'llm_multi_step'
  source: string; // 'oss' | 'pro:<vertical>'
  pinned?: boolean;
  active_hash?: string | null;
  inputs: SkillInputInfo[];
}

export interface SkillStepEstimate {
  id: string;
  tier: string;
  est_tokens: number;
}

export interface SkillEstimate {
  est_tokens: number;
  est_usd: number;
  est_seconds: number;
  steps: SkillStepEstimate[];
  over_cap: boolean;
}

export interface SkillDryRunStep {
  id: string;
  kind: string;
  tier: string;
  detail?: string | null;
}

export interface SkillDryRunItemRef {
  id: string;
  found: boolean;
  title?: string | null;
  chars: number;
}

export interface SkillDryRunResult {
  skill: SkillInfo;
  valid_inputs: boolean;
  can_run: boolean;
  has_llm: boolean;
  member_required: boolean;
  privacy_llm_required: boolean;
  referenced_items: SkillDryRunItemRef[];
  steps: SkillDryRunStep[];
  estimate: SkillEstimate;
  blockers: string[];
  warnings: string[];
}

export interface SkillVersionSnapshot {
  skill_id: string;
  version: string;
  title: string;
  source: string;
  hash: string;
  captured_at: string;
  note?: string | null;
}

export interface SkillVersionEntry {
  skill_id: string;
  current: SkillVersionSnapshot;
  active?: SkillVersionSnapshot | null;
  history: SkillVersionSnapshot[];
  drift: boolean;
}

export interface SkillRunResult {
  skill_id: string;
  filename: string;
  format: string;
  mime: string;
  size_bytes: number;
  artifact_base64: string;
  artifact: unknown;
  token_bill: unknown;
  warnings: string[];
  partial: boolean;
}

export class SkillError extends Error {
  constructor(
    public status: number,
    public code: string,
    message: string,
  ) {
    super(message);
    this.name = 'SkillError';
  }
}

async function parseError(res: Response, fallback: string): Promise<SkillError> {
  let code = 'skill-failed';
  let message = fallback;
  try {
    const body = (await res.json()) as { code?: string; error?: string };
    if (body.code) code = body.code;
    if (body.error) message = body.error;
  } catch {
    /* non-JSON */
  }
  return new SkillError(res.status, code, message);
}

/** GET the list of registered runtime skills (🆓). */
export async function listSkills(): Promise<SkillInfo[]> {
  const res = await fetch('/api/v1/skill-runtime/skills', { headers: authHeaders() });
  if (!res.ok) throw await parseError(res, `list skills failed (HTTP ${res.status})`);
  const body = (await res.json()) as { skills: SkillInfo[] };
  return body.skills;
}

/** POST a static (🆓, no-LLM) cost estimate for a skill + inputs. */
export async function estimateSkill(
  id: string,
  inputs: Record<string, unknown>,
): Promise<SkillEstimate> {
  const res = await fetch(`/api/v1/skill-runtime/skills/${encodeURIComponent(id)}/estimate`, {
    method: 'POST',
    headers: authHeaders(),
    body: JSON.stringify({ inputs }),
  });
  if (!res.ok) throw await parseError(res, `estimate failed (HTTP ${res.status})`);
  return (await res.json()) as SkillEstimate;
}

/** POST a no-side-effect dry-run plan (no LLM / no agent / no file export). */
export async function dryRunSkill(
  id: string,
  inputs: Record<string, unknown>,
): Promise<SkillDryRunResult> {
  const res = await fetch(`/api/v1/skill-runtime/skills/${encodeURIComponent(id)}/dry-run`, {
    method: 'POST',
    headers: authHeaders(),
    body: JSON.stringify({ inputs }),
  });
  if (!res.ok) throw await parseError(res, `dry-run failed (HTTP ${res.status})`);
  return (await res.json()) as SkillDryRunResult;
}

export async function listSkillVersions(): Promise<SkillVersionEntry[]> {
  const res = await fetch('/api/v1/skill-runtime/versions', { headers: authHeaders() });
  if (!res.ok) throw await parseError(res, `list skill versions failed (HTTP ${res.status})`);
  const body = (await res.json()) as { skills: SkillVersionEntry[] };
  return body.skills;
}

export async function captureSkillSnapshot(
  id: string,
  note = '',
  activate = false,
): Promise<SkillVersionEntry> {
  const res = await fetch(`/api/v1/skill-runtime/skills/${encodeURIComponent(id)}/versions/snapshot`, {
    method: 'POST',
    headers: authHeaders(),
    body: JSON.stringify({ note, activate }),
  });
  if (!res.ok) throw await parseError(res, `capture snapshot failed (HTTP ${res.status})`);
  return (await res.json()) as SkillVersionEntry;
}

export async function activateSkillSnapshot(
  id: string,
  hash: string,
): Promise<SkillVersionEntry> {
  const res = await fetch(`/api/v1/skill-runtime/skills/${encodeURIComponent(id)}/versions/activate`, {
    method: 'POST',
    headers: authHeaders(),
    body: JSON.stringify({ hash }),
  });
  if (!res.ok) throw await parseError(res, `activate snapshot failed (HTTP ${res.status})`);
  return (await res.json()) as SkillVersionEntry;
}

export async function clearSkillSnapshot(id: string): Promise<SkillVersionEntry> {
  const res = await fetch(`/api/v1/skill-runtime/skills/${encodeURIComponent(id)}/versions/active`, {
    method: 'DELETE',
    headers: authHeaders(),
  });
  if (!res.ok) throw await parseError(res, `clear snapshot failed (HTTP ${res.status})`);
  return (await res.json()) as SkillVersionEntry;
}

/** Decode the base64 artifact and trigger a browser download. */
function downloadBase64(b64: string, filename: string, mime: string): void {
  const bin = atob(b64);
  const bytes = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
  const blob = new Blob([bytes], { type: mime });
  const url = URL.createObjectURL(blob);
  const a = document.createElement('a');
  a.href = url;
  a.download = filename;
  document.body.appendChild(a);
  a.click();
  a.remove();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}

/** POST a skill run (💰). `confirm_cost` MUST be true. Downloads the produced artifact. */
export async function runSkill(
  id: string,
  inputs: Record<string, unknown>,
): Promise<SkillRunResult> {
  const res = await fetch(`/api/v1/skill-runtime/skills/${encodeURIComponent(id)}/run`, {
    method: 'POST',
    headers: authHeaders(),
    body: JSON.stringify({ inputs, confirm_cost: true }),
  });
  if (!res.ok) throw await parseError(res, `skill run failed (HTTP ${res.status})`);
  const result = (await res.json()) as SkillRunResult;
  downloadBase64(result.artifact_base64, result.filename, result.mime);
  return result;
}
