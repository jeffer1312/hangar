// Integrações MCP externas (`/api/integrations/*`) e aprovações SSE (`integrations_approvals`).
// Segredos viajam somente na escrita (POST/PATCH) e nunca retornam em listagens.
import { integrationsFetchForServer } from './api';
import type { Server } from './servers';
import * as m from './paraglide/messages';

export type PresetAuthKind = 'oauth_dcr' | 'oauth_manual_client' | 'oauth_dcr_or_bearer';

export interface IntegrationSetupGuide {
  title: string;
  steps: string[];
  docs_hint?: string | null;
}

export interface IntegrationPreset {
  id: string;
  name: string;
  category: string;
  description: string;
  url: string;
  url_editable: boolean;
  auth_kind: PresetAuthKind;
  default_scopes: string[];
  requires_loopback_redirect: boolean;
  setup_guide?: IntegrationSetupGuide | null;
}

export type IntegrationTransport =
  | { kind: 'remote_http'; url: string; header_names: string[] }
  | { kind: 'local_stdio'; command: string[]; cwd?: string | null; env_keys: string[] };

export type IntegrationTransportInput =
  | { kind: 'remote_http'; url: string; header_names?: string[] }
  | { kind: 'local_stdio'; command: string[]; cwd?: string | null; env_keys?: string[] };

export type IntegrationAuthConfig =
  | { kind: 'none' }
  | { kind: 'bearer' }
  | { kind: 'oauth_dcr' }
  | { kind: 'oauth_manual_client'; client_id: string; scopes: string[] };

export type PermissionPolicy = 'read_free_ask_write' | 'allow_all' | 'read_only';
export type IntegrationStatus = 'ready' | 'auth_required' | 'auth_expired' | 'disabled' | 'error';
export type ToolAccess = 'read' | 'allowed_always' | 'ask_write' | 'blocked';

export interface IntegrationView {
  id: string;
  name: string;
  preset?: string | null;
  enabled: boolean;
  transport: IntegrationTransport;
  auth: IntegrationAuthConfig;
  policy: PermissionPolicy;
  pinned: boolean;
  projects: string[];
  allowed_tools: string[];
  created_at: number;
  updated_at: number;
  status: IntegrationStatus;
  last_error?: string;
}

export interface IntegrationToolSummary {
  name: string;
  description?: string | null;
  input_schema: unknown;
  read_only_hint?: boolean | null;
  effective_access: ToolAccess;
}

export interface IntegrationSecretsPatch {
  bearer_token?: string | null;
  headers?: Record<string, string | null> | null;
  oauth_client_secret?: string | null;
  client_secret?: string | null;
  env?: Record<string, string | null> | null;
}

export interface CreateIntegrationInput {
  id: string;
  name: string;
  preset?: string | null;
  enabled?: boolean;
  transport: IntegrationTransportInput;
  auth: IntegrationAuthConfig;
  policy?: PermissionPolicy;
  pinned?: boolean;
  projects?: string[];
  allowed_tools?: string[];
  secrets?: IntegrationSecretsPatch;
}

export interface UpdateIntegrationInput {
  name?: string;
  preset?: string | null;
  enabled?: boolean;
  transport?: IntegrationTransportInput;
  auth?: IntegrationAuthConfig;
  policy?: PermissionPolicy;
  pinned?: boolean;
  projects?: string[];
  allowed_tools?: string[];
  secrets?: IntegrationSecretsPatch | null;
}

export type OAuthOrigin = 'desktop' | 'mobile';

export interface OAuthStartInput {
  origin_client: OAuthOrigin;
  public_base_url?: string;
  cimd_client_id_url?: string;
}

export interface OAuthStartResult {
  mode: 'open_url' | 'handoff_desktop';
  authorize_url: string;
  redirect_uri: string;
  reason?: string;
}

export type TestIntegrationResult =
  | { status: 'ready'; tools: IntegrationToolSummary[] }
  | { status: 'auth_required' | 'auth_expired'; tools: IntegrationToolSummary[]; reason?: string };

export type IntegrationApprovalDecision = 'allow_once' | 'allow_always' | 'deny';

export interface IntegrationApprovalRequest {
  id: string;
  interaction_id: string;
  origin: string;
  integration_id: string;
  integration_name: string;
  tool: string;
  arguments_preview: string;
  reason: string;
  expires_at: number;
}

export interface IntegrationApprovalEvent {
  requests: IntegrationApprovalRequest[];
}

const READ_MS = 15_000;
const OAUTH_MS = 30_000;
const TEST_MS = 45_000;
// Leituras e gravações de segredos podem aguardar o chaveiro do sistema operacional.
const KEYRING_MS = 45_000;

const integrationPath = (id: string) => `/api/integrations/${encodeURIComponent(id)}`;
const approvalPath = (id: string) => `/api/integrations/approvals/${encodeURIComponent(id)}`;

const ERROR_TEXT: Record<string, () => string> = {
  auth_required: m.integrations_err_auth_required,
  auth_expired: m.integrations_err_auth_expired,
  integration_owner_only: m.integrations_err_owner_only,
  integration_disabled: m.integrations_err_disabled,
  integration_already_exists: m.integrations_err_already_exists,
  integration_secrets_key_lost: m.integrations_err_keyring,
  integration_secrets_unavailable: m.integrations_err_keyring,
  approval_stale: m.integrations_err_stale,
  approval_answered: m.integrations_err_answered,
  approval_lost: m.integrations_approval_lost,
  approval_not_found: m.integrations_approval_lost,
};

/** Traduz os códigos de erro do servidor para integrações; código desconhecido retorna `undefined`. */
export function integrationFailureText(err: unknown): string | undefined {
  const { code, envelope, message } = (err ?? {}) as {
    code?: unknown;
    envelope?: { msg?: unknown };
    message?: unknown;
  };
  if (typeof code !== 'string' || !code) return undefined;
  if (code === 'approval_conflict') {
    const raw = typeof envelope?.msg === 'string' ? envelope.msg : typeof message === 'string' ? message : '';
    return /atualiz|stale/i.test(raw) ? m.integrations_err_stale() : m.integrations_err_answered();
  }
  return Object.prototype.hasOwnProperty.call(ERROR_TEXT, code) ? ERROR_TEXT[code]?.() : undefined;
}

/** Escolhe a URL HTTPS pública do servidor para o callback OAuth vindo do celular. */
export function resolveMobilePublicBaseUrl(server: Pick<Server, 'baseUrl' | 'remote'>): string | undefined {
  for (const candidate of [server.remote?.url, server.baseUrl]) {
    if (!candidate) continue;
    const trimmed = candidate.trim();
    try {
      const parsed = new URL(trimmed);
      if (parsed.protocol === 'https:' && parsed.hostname && !parsed.username && !parsed.password) {
        return trimmed.replace(/\/+$/, '');
      }
    } catch { /* endereço inválido não pode virar redirect_uri */ }
  }
  return undefined;
}

const isCard = (v: unknown): v is IntegrationApprovalRequest => {
  if (!v || typeof v !== 'object' || Array.isArray(v)) return false;
  const r = v as Record<string, unknown>;
  return typeof r.id === 'string' && r.id.length > 0
    && typeof r.interaction_id === 'string' && r.interaction_id.length > 0
    && typeof r.origin === 'string'
    && typeof r.integration_id === 'string' && r.integration_id.length > 0
    && typeof r.integration_name === 'string'
    && typeof r.tool === 'string' && r.tool.length > 0
    && typeof r.arguments_preview === 'string'
    && typeof r.reason === 'string'
    && typeof r.expires_at === 'number' && Number.isFinite(r.expires_at);
};

/** Evento SSE `integrations_approvals` torto devolve `null`; cartão malformado sai da lista. */
export function parseIntegrationApprovalEvent(raw: string): IntegrationApprovalEvent | null {
  let parsed: unknown;
  try { parsed = JSON.parse(raw); } catch { return null; }
  if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) return null;
  const { requests } = parsed as { requests?: unknown };
  if (!Array.isArray(requests)) return null;
  return {
    requests: requests.filter(isCard).map((r) => ({
      id: r.id,
      interaction_id: r.interaction_id,
      origin: r.origin,
      integration_id: r.integration_id,
      integration_name: r.integration_name,
      tool: r.tool,
      arguments_preview: r.arguments_preview,
      reason: r.reason,
      expires_at: r.expires_at,
    })),
  };
}

export async function listCatalog(s: Server): Promise<IntegrationPreset[]> {
  const res = await integrationsFetchForServer<{ presets?: IntegrationPreset[] }>(s, '/api/integrations/catalog', undefined, READ_MS);
  return Array.isArray(res?.presets) ? res.presets : [];
}

export async function listIntegrations(s: Server): Promise<IntegrationView[]> {
  const res = await integrationsFetchForServer<{ integrations?: IntegrationView[] }>(s, '/api/integrations', undefined, KEYRING_MS);
  return Array.isArray(res?.integrations) ? res.integrations : [];
}

export async function createIntegration(s: Server, input: CreateIntegrationInput): Promise<IntegrationView> {
  const res = await integrationsFetchForServer<{ integration: IntegrationView }>(
    s,
    '/api/integrations',
    { method: 'POST', body: JSON.stringify(input) },
    KEYRING_MS,
  );
  return res.integration;
}

export async function updateIntegration(s: Server, id: string, patch: UpdateIntegrationInput): Promise<IntegrationView> {
  const res = await integrationsFetchForServer<{ integration: IntegrationView }>(
    s,
    integrationPath(id),
    { method: 'PATCH', body: JSON.stringify(patch) },
    KEYRING_MS,
  );
  return res.integration;
}

export function deleteIntegration(s: Server, id: string): Promise<{ ok: boolean; id: string }> {
  return integrationsFetchForServer<{ ok: boolean; id: string }>(s, integrationPath(id), { method: 'DELETE' }, KEYRING_MS);
}

export function startIntegrationOAuth(s: Server, id: string, input: OAuthStartInput): Promise<OAuthStartResult> {
  return integrationsFetchForServer<OAuthStartResult>(
    s,
    `${integrationPath(id)}/oauth/start`,
    { method: 'POST', body: JSON.stringify(input) },
    OAUTH_MS,
  );
}

export function openIntegrationOAuthOnDesktop(s: Server, id: string, authorizeUrl?: string): Promise<{ ok: boolean }> {
  return integrationsFetchForServer<{ ok: boolean }>(
    s,
    `${integrationPath(id)}/oauth/open-on-desktop`,
    { method: 'POST', body: JSON.stringify(authorizeUrl ? { authorize_url: authorizeUrl } : {}) },
    OAUTH_MS,
  );
}

export async function testIntegration(s: Server, id: string): Promise<TestIntegrationResult> {
  try {
    const res = await integrationsFetchForServer<{ status: 'ready'; tools: IntegrationToolSummary[] }>(
      s,
      `${integrationPath(id)}/test`,
      { method: 'POST' },
      TEST_MS,
    );
    return { status: 'ready', tools: Array.isArray(res?.tools) ? res.tools : [] };
  } catch (err) {
    const { status, code, envelope, message } = (err ?? {}) as {
      status?: unknown;
      code?: unknown;
      envelope?: { msg?: unknown };
      message?: unknown;
    };
    // 424 evita confundir token externo ausente/expirado com 401 da sessão do Hangar.
    if (status === 424 && (code === 'auth_required' || code === 'auth_expired')) {
      const reason = typeof envelope?.msg === 'string' && envelope.msg
        ? envelope.msg
        : typeof message === 'string'
          ? message.replace(/^424:\s*/, '')
          : undefined;
      return { status: code, tools: [], ...(reason ? { reason } : {}) };
    }
    throw err;
  }
}

export async function listIntegrationTools(s: Server, id: string): Promise<IntegrationToolSummary[]> {
  const res = await integrationsFetchForServer<{ tools?: IntegrationToolSummary[] }>(
    s,
    `${integrationPath(id)}/tools`,
    undefined,
    READ_MS,
  );
  return Array.isArray(res?.tools) ? res.tools : [];
}

export function answerIntegrationApproval(
  s: Server,
  requestId: string,
  answer: { interaction_id: string; decision: IntegrationApprovalDecision },
): Promise<{ ok: boolean; decision: IntegrationApprovalDecision }> {
  return integrationsFetchForServer<{ ok: boolean; decision: IntegrationApprovalDecision }>(
    s,
    approvalPath(requestId),
    { method: 'POST', body: JSON.stringify(answer) },
    READ_MS,
  );
}

export function cancelIntegrationApproval(s: Server, requestId: string): Promise<{ ok: boolean }> {
  return integrationsFetchForServer<{ ok: boolean }>(s, approvalPath(requestId), { method: 'DELETE' }, READ_MS);
}
