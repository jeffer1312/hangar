import { comTeto, errorDetail, probeServerResponse } from '@hangar/core';
import { useServers } from '../../stores/servers';

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
