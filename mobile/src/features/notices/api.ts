import { comTeto, errorDetail, probeServerResponse } from '@hangar/core';
import { useServers } from '../../stores/servers';
import { parseNoticeIgnores } from './notices';

/** Ignora ou volta a mostrar um aviso na máquina. Devolve a lista nova; `null` quando o corpo veio torto (vale o evento). */
export async function putNoticeIgnore(serverId: string, key: string, ignored: boolean): Promise<string[] | null> {
  const server = useServers.getState().servers.find((s) => s.id === serverId);
  if (!server) throw new Error(`servidor ${serverId} não está na lista`);
  const response = await probeServerResponse(server, '/api/notice-ignores', {
    method: 'PUT', signal: comTeto(undefined, 15_000), body: JSON.stringify({ key, ignored }),
  });
  if (!response.ok) throw new Error(`${response.status}: ${await errorDetail(response)}`);
  // O servidor já gravou: corpo ilegível não vira "falhou", a lista chega pelo evento `notice_ignores`.
  return parseNoticeIgnores(await response.text());
}
