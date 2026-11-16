import { comTeto, errorDetail, probeServerResponse } from '@hangar/core';
import { useServers } from '../../stores/servers';

function serverOf(serverId: string) {
  const server = useServers.getState().servers.find((s) => s.id === serverId);
  if (!server) throw new Error(`servidor ${serverId} não está na lista`);
  return server;
}

async function post(serverId: string, path: string, body: unknown, seconds: number): Promise<unknown> {
  const response = await probeServerResponse(serverOf(serverId), path, {
    method: 'POST', signal: comTeto(undefined, seconds * 1000), body: body === undefined ? undefined : JSON.stringify(body),
  });
  if (!response.ok) throw new Error(`${response.status}: ${await errorDetail(response)}`);
  return response.json();
}

/** Reinstala pela rota de plugins da máquina; a CLI pode demorar. Falha da CLI vem com `result.outcome = "failed"`. */
export async function reinstallPlugin(serverId: string, plugin: string): Promise<void> {
  const body = await post(serverId, `/api/plugins/${encodeURIComponent(plugin)}/install`, undefined, 240) as
    { result?: { outcome?: string; message?: string } };
  if (body.result?.outcome === 'failed') throw new Error(body.result.message ?? 'falhou');
}

export type FixRequest = { notice_id: string; subject: string; detail: string; blocking: boolean };
export type FixPreview = { subject: string; detail: string; files: string[]; no_hooks: boolean };

const fixPath = (session: string, tail: string) => `/api/sessions/${encodeURIComponent(session)}/notice/${tail}`;

export const fixPreview = (serverId: string, session: string, request: FixRequest) =>
  post(serverId, fixPath(session, 'fix/preview'), request, 30) as Promise<FixPreview>;

/** Abre a sessão de conserto; o andamento chega pela nota que o servidor grava na conversa. */
export const startFix = (serverId: string, session: string, request: FixRequest) =>
  post(serverId, fixPath(session, 'fix'), request, 120) as Promise<{ child: string }>;

/**
 * Ignora ou volta a mostrar um aviso na máquina. A lista nova chega pelo evento `notice_ignores`, em ordem: a resposta
 * de um PUT anterior que chegasse depois traria de volta uma lista velha.
 */
export async function putNoticeIgnore(serverId: string, key: string, ignored: boolean): Promise<void> {
  const server = useServers.getState().servers.find((s) => s.id === serverId);
  if (!server) throw new Error(`servidor ${serverId} não está na lista`);
  const response = await probeServerResponse(server, '/api/notice-ignores', {
    method: 'PUT', signal: comTeto(undefined, 15_000), body: JSON.stringify({ key, ignored }),
  });
  if (!response.ok) throw new Error(`${response.status}: ${await errorDetail(response)}`);
}
