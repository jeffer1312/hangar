// Integrações MCP externas (`/api/integrations/*`) e aprovações SSE (`integrations_approvals`).
// Segredos viajam somente na escrita (POST/PATCH) e nunca retornam em listagens.
import { integrationsFetchForServer } from './api';
import type { LanInfo, Server } from './servers';
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
  add_allowed_tools?: string[];
  remove_allowed_tools?: string[];
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

const MAX_URL_BYTES = 2048;
const MAX_HEADER_NAME_BYTES = 256;
// Cabeçalhos reservados pelo transporte MCP ou pelo fluxo de autenticação do Hangar.
export const RESERVED_HTTP_HEADERS: ReadonlySet<string> = new Set([
  'authorization',
  'accept',
  'mcp-session-id',
  'mcp-protocol-version',
  'last-event-id',
]);
const HEADER_NAME_RE = /^[!#$%&'*+\-.^_`|~0-9A-Za-z]+$/;

export type ParseHeadersErrorKind = 'invalid_line' | 'invalid_name' | 'reserved_name' | 'invalid_value';

export type ParseHeadersResult =
  | { ok: true; names: string[]; map: Record<string, string> }
  | { ok: false; reason: ParseHeadersErrorKind; name?: string };

export type ParseEnvErrorKind = 'invalid_line' | 'invalid_key' | 'invalid_value';

export type ParseEnvResult =
  | { ok: true; keys: string[]; map: Record<string, string> }
  | { ok: false; reason: ParseEnvErrorKind; key?: string };

export type ParseCommandArgsResult =
  | { ok: true; args: string[] }
  | { ok: false; unclosedQuote: '"' | "'" };

export interface IntegrationUrlValidationResult {
  valid: boolean;
  insecure?: boolean;
}

/** Divide texto por linha, aparando bordas e descartando linhas vazias. */
export function parseLines(raw: string): string[] {
  return raw.split('\n').map((s) => s.trim()).filter(Boolean);
}

/** Separa escopos OAuth por espaços ou vírgulas, ignorando itens vazios. */
export function parseScopes(raw: string): string[] {
  return raw.split(/[\s,]+/).map((s) => s.trim()).filter(Boolean);
}

/** Valida nome de cabeçalho HTTP segundo RFC 9110 (`tchar`) e recusa os reservados do MCP. */
export function isValidHeaderName(name: string): boolean {
  return name.length > 0
    && name.length <= MAX_HEADER_NAME_BYTES
    && HEADER_NAME_RE.test(name)
    && !RESERVED_HTTP_HEADERS.has(name.toLowerCase());
}

/** Lê linhas `Nome: Valor`, diferenciando linha malformada, nome inválido/reservado e valor inválido. */
export function parseHeaders(raw: string): ParseHeadersResult {
  const names: string[] = [];
  const map: Record<string, string> = {};
  for (const line of raw.split('\n')) {
    const trimmed = line.trim();
    if (!trimmed) continue;
    const sep = trimmed.indexOf(':');
    if (sep <= 0) return { ok: false, reason: 'invalid_line' };
    const name = trimmed.slice(0, sep).trim().toLowerCase();
    if (!name) return { ok: false, reason: 'invalid_line' };
    if (name.length > MAX_HEADER_NAME_BYTES || !HEADER_NAME_RE.test(name)) {
      return { ok: false, reason: 'invalid_name', name };
    }
    if (RESERVED_HTTP_HEADERS.has(name)) {
      return { ok: false, reason: 'reserved_name', name };
    }
    const val = trimmed.slice(sep + 1).trim();
    if (!val || /[\x00-\x08\x0a-\x1f\x7f]/.test(val)) {
      return { ok: false, reason: 'invalid_value', name };
    }
    if (!names.includes(name)) names.push(name);
    map[name] = val;
  }
  return { ok: true, names, map };
}

/** Lê linhas `CHAVE=VALOR`, preservando espaços finais do valor e removendo só `\r` final. */
export function parseEnvVars(raw: string): ParseEnvResult {
  const keys: string[] = [];
  const map: Record<string, string> = {};
  for (const line of raw.split('\n')) {
    // Só remove `\r` de quebra CRLF; espaços à direita do valor fazem parte da variável.
    const cleanLine = line.endsWith('\r') ? line.slice(0, -1) : line;
    if (!cleanLine.trim()) continue;
    const sep = cleanLine.indexOf('=');
    if (sep <= 0) return { ok: false, reason: 'invalid_line' };
    const key = cleanLine.slice(0, sep).trim();
    if (!key) return { ok: false, reason: 'invalid_line' };
    const val = cleanLine.slice(sep + 1);
    if (key.length > MAX_HEADER_NAME_BYTES || key.includes('=') || /[\s\x00-\x1f\x7f]/.test(key)) {
      return { ok: false, reason: 'invalid_key', key };
    }
    if (val.includes('\0')) {
      return { ok: false, reason: 'invalid_value', key };
    }
    if (!keys.includes(key)) keys.push(key);
    map[key] = val;
  }
  return { ok: true, keys, map };
}

/** Formata `command` de forma reversível para edição em texto único sem quebrar argumentos com espaços ou aspas. */
export function formatCommandArgs(command: string[]): string {
  return command
    .map((arg) => {
      if (arg.length > 0 && !/[\s"'\\]/.test(arg)) return arg;
      return `"${arg.replace(/\\/g, '\\\\').replace(/"/g, '\\"')}"`;
    })
    .join(' ');
}

const isEscapedInDoubleQuotes = (text: string, idx: number): boolean =>
  text[idx] === '\\' && idx + 1 < text.length && (text[idx + 1] === '"' || text[idx + 1] === '\\');

function splitUnquotedLines(
  trimmed: string,
): { ok: true; lines: string[]; multiLine: boolean } | { ok: false; unclosedQuote: '"' | "'" } {
  const lines: string[] = [];
  let cur = '';
  let quote: '"' | "'" | null = null;
  let multiLine = false;

  for (let i = 0; i < trimmed.length; i++) {
    const ch = trimmed[i];
    if (quote === '"') {
      if (isEscapedInDoubleQuotes(trimmed, i)) {
        cur += ch + trimmed[++i];
      } else {
        if (ch === '"') quote = null;
        cur += ch;
      }
    } else if (quote === "'") {
      if (ch === "'") quote = null;
      cur += ch;
    } else if (ch === '"' || ch === "'") {
      quote = ch;
      cur += ch;
    } else if (ch === '\\' && i + 1 < trimmed.length && /[ \t"'\\]/.test(trimmed[i + 1])) {
      cur += ch + trimmed[++i];
    } else if (ch === '\n') {
      multiLine = true;
      const line = cur.trim();
      if (line) lines.push(line);
      cur = '';
    } else {
      cur += ch;
    }
  }

  if (quote !== null) return { ok: false, unclosedQuote: quote };
  const last = cur.trim();
  if (last) lines.push(last);
  return { ok: true, lines, multiLine };
}

function tokenizeSingleLineArgs(trimmed: string): string[] {
  const args: string[] = [];
  let cur = '';
  let hasToken = false;
  let quote: '"' | "'" | null = null;

  for (let i = 0; i < trimmed.length; i++) {
    const ch = trimmed[i];
    if (quote === '"') {
      if (isEscapedInDoubleQuotes(trimmed, i)) {
        cur += trimmed[++i];
      } else if (ch === '"') {
        quote = null;
      } else {
        cur += ch;
      }
    } else if (quote === "'") {
      if (ch === "'") quote = null;
      else cur += ch;
    } else if (ch === '"' || ch === "'") {
      quote = ch;
      hasToken = true;
    } else if (ch === '\\' && i + 1 < trimmed.length && /[ \t"'\\]/.test(trimmed[i + 1])) {
      // Só escapa separadores e aspas fora das aspas para não estragar caminhos Windows sem escape.
      cur += trimmed[++i];
      hasToken = true;
    } else if (/\s/.test(ch)) {
      if (hasToken) {
        args.push(cur);
        cur = '';
        hasToken = false;
      }
    } else {
      cur += ch;
      hasToken = true;
    }
  }
  if (hasToken) args.push(cur);
  return args;
}

/** Separa comando e argumentos (uma linha por argumento ou linha única com aspas), recusando aspas sem fechamento. */
export function parseCommandArgs(raw: string): ParseCommandArgsResult {
  const trimmed = raw.trim();
  if (!trimmed) return { ok: true, args: [] };
  const split = splitUnquotedLines(trimmed);
  if (!split.ok) return split;
  if (split.multiLine) return { ok: true, args: split.lines };
  return { ok: true, args: tokenizeSingleLineArgs(trimmed) };
}

/** Valida URL HTTP(S) de transporte remoto e exige TLS ou loopback quando há autenticação ou cabeçalhos. */
export function isValidIntegrationUrl(
  url: string,
  opts: { hasAuthOrHeaders: boolean } = { hasAuthOrHeaders: false },
): IntegrationUrlValidationResult {
  const trimmed = url.trim();
  const byteLen = typeof TextEncoder !== 'undefined' ? new TextEncoder().encode(trimmed).length : trimmed.length;
  if (!trimmed || byteLen > MAX_URL_BYTES || /[\s\x00-\x1f\x7f]/.test(trimmed)) {
    return { valid: false };
  }
  let parsed: URL;
  try {
    parsed = new URL(trimmed);
  } catch {
    return { valid: false };
  }
  if (
    (parsed.protocol !== 'https:' && parsed.protocol !== 'http:')
    || !parsed.hostname
    || parsed.username
    || parsed.password
    || parsed.port === '0'
  ) {
    return { valid: false };
  }
  if (parsed.protocol === 'http:' && opts.hasAuthOrHeaders) {
    const host = parsed.hostname.toLowerCase();
    const isLoopback = host === 'localhost' || host === '127.0.0.1' || host === '::1' || host === '[::1]';
    if (!isLoopback) {
      return { valid: false, insecure: true };
    }
  }
  return { valid: true };
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
export function resolveMobilePublicBaseUrl(
  server: Pick<Server, 'baseUrl'> & {
    remote?: Pick<LanInfo, 'url'> | LanInfo;
    connect?: Pick<LanInfo, 'url'> | LanInfo;
  },
): string | undefined {
  for (const candidate of [server.remote?.url, server.connect?.url, server.baseUrl]) {
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
