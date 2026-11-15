import { comTeto, errorDetail, probeServerResponse } from '@hangar/core';
import { useServers } from '../../stores/servers';
import { setNoticeIgnores } from '../../stores/sessions';
import { parseNoticeIgnores } from './notices';

/** Ignora ou volta a mostrar um aviso na máquina; a lista nova vale na hora, sem esperar o evento. */
export async function putNoticeIgnore(serverId: string, key: string, ignored: boolean): Promise<void> {
  const server = useServers.getState().servers.find((s) => s.id === serverId);
  if (!server) throw new Error(serverId);
  const response = await probeServerResponse(server, '/api/notice-ignores', {
    method: 'PUT', signal: comTeto(undefined, 15_000), body: JSON.stringify({ key, ignored }),
  });
  if (!response.ok) throw new Error(`${response.status}: ${await errorDetail(response)}`);
  const keys = parseNoticeIgnores(await response.json());
  if (keys) setNoticeIgnores(serverId, keys);
}
