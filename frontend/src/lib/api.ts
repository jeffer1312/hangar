import { getBaseUrl, getToken } from './auth';
import type {
  SessionInfo,
  ChatEvent,
  CommandInfo,
  FsRoot,
  FsScanResult,
  FsScanError,
} from './types';

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
  if (!res.ok) {
    const text = await res.text().catch(() => res.statusText);
    throw new Error(`${res.status}: ${text}`);
  }
  return res.json() as Promise<T>;
}

export function getSessions(): Promise<SessionInfo[]> {
  return apiFetch<SessionInfo[]>('/api/sessions');
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

export function getHistory(name: string): Promise<ChatEvent[]> {
  return apiFetch<ChatEvent[]>(`/api/sessions/${encodeURIComponent(name)}/history`);
}

export function getCommands(name: string): Promise<CommandInfo[]> {
  return apiFetch<CommandInfo[]>(`/api/sessions/${encodeURIComponent(name)}/commands`);
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
