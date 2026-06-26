import { getBaseUrl, getToken, dropActiveServer } from './auth';
import type { Server } from './auth';
import type {
  SessionInfo,
  ChatEvent,
  CommandInfo,
  FsRoot,
  FsScanResult,
  FsScanError,
  WorkflowSummary,
  WorkflowDetail,
  WorkflowAgentDetail,
} from './types';

// URL da idx-ésima imagem (colada no terminal) de uma msg do transcript. `?token` porque <img> não
// manda header Authorization e cross-origin (multi-PC) não leva cookie — o backend aceita ?token.
export function transcriptImageUrl(name: string, id: string, idx: number): string {
  const t = getToken() ?? '';
  return `${getBaseUrl()}/api/sessions/${encodeURIComponent(name)}/transcript-image/${encodeURIComponent(id)}/${idx}?token=${encodeURIComponent(t)}`;
}

function authHeaders(): HeadersInit {
  const token = getToken();
  if (!token) return {};
  return { Authorization: `Bearer ${token}` };
}

async function apiFetch<T>(path: string, init?: RequestInit): Promise<T> {
  const base = getBaseUrl();
  const url = `${base}${path}`;
  const res = await fetch(url, {
    ...init,
    headers: {
      'Content-Type': 'application/json',
      ...authHeaders(),
      ...(init?.headers ?? {}),
    },
  });
  // Self-heal de token inválido/rotacionado: isAuthenticated() só checa se EXISTE token, nao
  // se vale. Sem isto, um token velho deixa o app travado em 401 (sessao "undefined"). Num 401
  // COM token salvo, limpamos a credencial e recarregamos -> cai no Login pra re-parear (QR).
  // O guard `getToken()` evita loop quando ja estamos deslogados (Login nao chama a API).
  if (res.status === 401 && getToken()) {
    dropActiveServer();
    if (typeof window !== 'undefined') window.location.reload();
    throw new Error('401: sessão expirada — faça login novamente');
  }
  if (!res.ok) {
    const text = await res.text().catch(() => res.statusText);
    throw new Error(`${res.status}: ${text}`);
  }
  return res.json() as Promise<T>;
}

export function getSessions(): Promise<SessionInfo[]> {
  return apiFetch<SessionInfo[]>('/api/sessions');
}

// Lista sessões de UM servidor específico (baseUrl+token explícitos), sem mexer no ativo. Usado
// pela visão agregada: cada servidor é consultado direto, então um ativo "global" não importa aqui.
async function fetchSessionsForServer(s: Server): Promise<SessionInfo[]> {
  const res = await fetch(`${s.baseUrl}/api/sessions`, {
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${s.token}` },
  });
  if (!res.ok) throw new Error(`${res.status}`);
  return res.json() as Promise<SessionInfo[]>;
}

// Resultado por-servidor: sessões OU erro (servidor offline/token ruim). Nunca rejeita —
// uma falha num servidor não pode esconder os outros nem o 401 não força logout aqui.
export interface ServerSessions {
  server: Server;
  sessions: SessionInfo[] | null;
  error: string | null;
}

export async function getAllSessions(servers: Server[]): Promise<ServerSessions[]> {
  return Promise.all(
    servers.map(async (server): Promise<ServerSessions> => {
      try {
        return { server, sessions: await fetchSessionsForServer(server), error: null };
      } catch (e) {
        return { server, sessions: null, error: e instanceof Error ? e.message : 'erro' };
      }
    }),
  );
}

export function createSession(name: string, cwd?: string): Promise<SessionInfo> {
  return apiFetch<SessionInfo>('/api/sessions', {
    method: 'POST',
    body: JSON.stringify({ name, cwd }),
  });
}

export async function deleteSession(name: string): Promise<void> {
  await apiFetch<{ ok: boolean }>(`/api/sessions/${encodeURIComponent(name)}`, {
    method: 'DELETE',
  });
}

// Renomeia a sessao do tmux. Devolve o nome final (sanitizado pelo backend).
export async function renameSession(name: string, newName: string): Promise<{ ok: boolean; name: string }> {
  return apiFetch<{ ok: boolean; name: string }>(`/api/sessions/${encodeURIComponent(name)}/rename`, {
    method: 'POST',
    body: JSON.stringify({ new: newName }),
  });
}

export function getHistory(name: string): Promise<ChatEvent[]> {
  return apiFetch<ChatEvent[]>(`/api/sessions/${encodeURIComponent(name)}/history`);
}

export function getCommands(name: string): Promise<CommandInfo[]> {
  return apiFetch<CommandInfo[]>(`/api/sessions/${encodeURIComponent(name)}/commands`);
}

// Workflows: lista de runs + detalhe (fases + agentes) — lidos dos arquivos do run no disco.
export function getWorkflows(name: string): Promise<WorkflowSummary[]> {
  return apiFetch<WorkflowSummary[]>(`/api/sessions/${encodeURIComponent(name)}/workflows`);
}

export function getWorkflow(name: string, runId: string): Promise<WorkflowDetail> {
  return apiFetch<WorkflowDetail>(`/api/sessions/${encodeURIComponent(name)}/workflows/${encodeURIComponent(runId)}`);
}

export function getWorkflowAgent(name: string, runId: string, agentId: string): Promise<WorkflowAgentDetail> {
  return apiFetch<WorkflowAgentDetail>(`/api/sessions/${encodeURIComponent(name)}/workflows/${encodeURIComponent(runId)}/agents/${encodeURIComponent(agentId)}`);
}

// Raízes liberadas do scanner (chips no topo do FolderScanner).
export function getRoots(): Promise<FsRoot[]> {
  return apiFetch<FsRoot[]>('/api/fs/roots');
}

/**
 * Lista os subdiretórios imediatos de `path` (default = `root`) dentro da raiz.
 * Rejeições de fronteira do backend (403 raiz não liberada, 400 caminho inválido,
 * 404 ausente) viram um FsScanResult com `error` tipado: a UI tem UM caminho de
 * renderização (lê `result.error`), em vez de misturar throws com campos. Apenas 401
 * borbulha (problema de auth, não de varredura).
 */
export async function scanDir(root: string, path?: string): Promise<FsScanResult> {
  const qs = new URLSearchParams({ root });
  if (path) qs.set('path', path);
  try {
    return await apiFetch<FsScanResult>(`/api/fs/scan?${qs.toString()}`);
  } catch (e) {
    if (!(e instanceof Error)) throw e;
    const status = parseInt(e.message, 10);
    if (status === 401) throw e;
    const map: Record<number, FsScanError> = {
      400: 'invalid_path',
      403: 'root_not_allowed',
      404: 'not_found',
    };
    return { entries: [], error: map[status] ?? 'unknown' };
  }
}

export async function sendInput(name: string, text: string): Promise<void> {
  await apiFetch<{ ok: boolean }>(`/api/sessions/${encodeURIComponent(name)}/input`, {
    method: 'POST',
    body: JSON.stringify({ text }),
  });
}

/**
 * Envia os bytes crus de uma imagem pra sessao (sem multipart). O backend salva e devolve o
 * path; o app depois manda a legenda + path pelo /input. 401 -> self-heal (igual apiFetch).
 */
export async function uploadImage(name: string, file: File): Promise<{ path: string }> {
  const base = getBaseUrl();
  const res = await fetch(`${base}/api/sessions/${encodeURIComponent(name)}/upload`, {
    method: 'POST',
    headers: {
      ...authHeaders(),
      'Content-Type': file.type || 'application/octet-stream',
    },
    body: file,
  });
  if (res.status === 401 && getToken()) {
    dropActiveServer();
    if (typeof window !== 'undefined') window.location.reload();
    throw new Error('401: sessão expirada — faça login novamente');
  }
  if (!res.ok) {
    const text = await res.text().catch(() => res.statusText);
    throw new Error(`${res.status}: ${text}`);
  }
  return res.json() as Promise<{ path: string }>;
}

export async function selectOption(name: string, option: number): Promise<void> {
  await apiFetch<{ ok: boolean }>(`/api/sessions/${encodeURIComponent(name)}/select`, {
    method: 'POST',
    body: JSON.stringify({ option }),
  });
}

export async function interrupt(name: string): Promise<void> {
  await apiFetch<{ ok: boolean }>(`/api/sessions/${encodeURIComponent(name)}/interrupt`, {
    method: 'POST',
    body: JSON.stringify({}),
  });
}

export interface ModelEffortBody {
  model?: string; // 'default' | 'opus' | 'sonnet' | 'haiku'
  effort?: string; // low | medium | high | xhigh | max | ultracode
  scope: 'session' | 'default';
}

/**
 * Applies a model/effort switch by driving Claude Code's interactive `/model` picker.
 * scope 'session' presses `s` (current session only); 'default' presses Enter (saved default).
 * Unlike the old full-arg `/model <arg>` command, scope 'session' does NOT change the user's
 * default for new sessions.
 */
export async function setModelEffort(name: string, body: ModelEffortBody): Promise<void> {
  await apiFetch<{ ok: boolean }>(`/api/sessions/${encodeURIComponent(name)}/model-effort`, {
    method: 'POST',
    body: JSON.stringify(body),
  });
}

/**
 * Opens an SSE stream for the given session.
 * In production (same-origin), the auth cookie is sent automatically.
 * In dev, appends ?token= as fallback.
 */
export function openEventStream(name: string): EventSource {
  const base = getBaseUrl();
  const token = getToken();
  const path = `/api/sessions/${encodeURIComponent(name)}/events`;

  // Use ?token param only in dev (different origin) or when no cookie is set
  const isSameOrigin = !base || base === window.location.origin;
  const url = isSameOrigin
    ? `${base}${path}`
    : `${base}${path}?token=${encodeURIComponent(token ?? '')}`;

  return new EventSource(url, { withCredentials: isSameOrigin });
}
