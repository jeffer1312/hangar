import { apiFetchForServer, comTeto, errorDetail, probeServerResponse } from './api';
import type { AntigravityLoginAttempt } from './credenciais';
import type { Server } from './servers';

export interface ClaudeLoginStep {
  etapa: 'idle' | 'aguardando' | 'concluido';
  url: string | null;
  email: string | null;
  plano: string | null;
}

export interface ClaudeLoginResult {
  ok: true;
  email?: string | null;
  plano?: string | null;
}

function nullableString(value: unknown): value is string | null {
  return value === null || typeof value === 'string';
}

async function requestLogin<T>(server: Server, label: string, suffix: string, init: RequestInit): Promise<T> {
  const res = await probeServerResponse(server, `/api/conta-estado/${encodeURIComponent(label)}/login${suffix}`, init);
  const body: unknown = res.ok ? await res.clone().json().catch((error: unknown) => {
    if (init.signal?.aborted) throw init.signal.reason ?? error;
    return null;
  }) : null;
  const data = body && typeof body === 'object' && !Array.isArray(body) ? body as Record<string, unknown> : null;
  const valid = data && (suffix === '/passo'
    ? ['idle', 'aguardando', 'concluido'].includes(data.etapa as string)
      && nullableString(data.url) && nullableString(data.email) && nullableString(data.plano)
    : data.ok === true && (data.email === undefined || nullableString(data.email))
      && (data.plano === undefined || nullableString(data.plano)));
  if (!res.ok || !valid) throw Object.assign(new Error(`${res.status}: ${await errorDetail(res)}`), { status: res.status });
  return body as T;
}

export function startClaudeAccountLoginForServer(server: Server, label: string): Promise<ClaudeLoginResult> {
  return requestLogin(server, label, '', { method: 'POST', signal: AbortSignal.timeout(30_000) });
}

export function getClaudeAccountLoginStepForServer(server: Server, label: string, signal?: AbortSignal): Promise<ClaudeLoginStep> {
  return requestLogin(server, label, '/passo', { method: 'GET', signal: comTeto(signal, 10_000) });
}

export function submitClaudeAccountLoginCodeForServer(server: Server, label: string, codigo: string): Promise<ClaudeLoginResult> {
  return requestLogin(server, label, '/codigo', {
    method: 'POST', body: JSON.stringify({ codigo }), signal: AbortSignal.timeout(310_000),
  });
}

export function cancelClaudeAccountLoginForServer(server: Server, label: string): Promise<ClaudeLoginResult> {
  return requestLogin(server, label, '/cancelar', { method: 'POST', signal: AbortSignal.timeout(15_000) });
}

// Login da conta Google do Antigravity: o servidor roda o `agy` de login, mostra o link do Google e
// escreve no processo o código que a pessoa colar. Uma tentativa por conta, pelo `attempt_id`.
function antigravityLoginPath(id: string, suffix = ''): string {
  return `/api/antigravity-contas/${encodeURIComponent(id)}/login${suffix}`;
}

export function startAntigravityLoginForServer(server: Server, id: string): Promise<AntigravityLoginAttempt> {
  return apiFetchForServer(server, antigravityLoginPath(id), { method: 'POST' }, 30_000);
}

export function getAntigravityLoginForServer(server: Server, id: string, attemptId: string, signal?: AbortSignal): Promise<AntigravityLoginAttempt> {
  return apiFetchForServer(server, `${antigravityLoginPath(id)}?attempt_id=${encodeURIComponent(attemptId)}`, { signal: comTeto(signal, 10_000) }, 10_000);
}

/** `codigo` é o código que a página do Antigravity mostra, ou o endereço dela inteiro: o servidor confere os dois. */
export function submitAntigravityLoginCodeForServer(server: Server, id: string, attemptId: string, codigo: string): Promise<{ ok: true }> {
  return apiFetchForServer(server, antigravityLoginPath(id, '/codigo'), {
    method: 'POST', body: JSON.stringify({ attempt_id: attemptId, codigo }),
  }, 30_000);
}

export function cancelAntigravityLoginForServer(server: Server, id: string, attemptId: string): Promise<{ ok: true }> {
  return apiFetchForServer(server, `${antigravityLoginPath(id)}?attempt_id=${encodeURIComponent(attemptId)}`, { method: 'DELETE' }, 15_000);
}

/** Sai da conta pelo próprio agy; a pasta e as conversas ficam. */
export function signOutAntigravityAccountForServer(server: Server, id: string): Promise<{ ok: true }> {
  return apiFetchForServer(server, `/api/antigravity-contas/${encodeURIComponent(id)}/logout`, { method: 'POST' }, 30_000);
}

/** O link que a tela abre ou copia: só https do login do Google, sem usuário ou senha embutidos. */
export function antigravityLoginLink(link: string | null | undefined): string | null {
  try {
    const url = new URL(link ?? '');
    return url.protocol === 'https:' && url.hostname === 'accounts.google.com' && !url.username && !url.password ? url.href : null;
  } catch {
    return null;
  }
}
