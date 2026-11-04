// "Em todas as minhas máquinas": repete uma gravação em cada servidor próprio e diz, por máquina,
// o que não foi aplicado e por quê. Sem fila durável: quem não respondeu é tentado de novo pela tela.
import type { Server } from './servers';
import { ownUpdatableServers } from './updateAll';
import { codexOpcoes, getHarnessesForServer } from './api';

/** `old` = servidor sem a rota (404); `absent` = o alvo (motor, Codex) não existe nesta máquina;
 *  `failed` = o servidor respondeu e recusou. */
export type ApplyStatus = 'ok' | 'offline' | 'old' | 'absent' | 'failed';
export interface ApplyResult { serverId: string; status: ApplyStatus; error?: string }

/** `fn` devolve `'absent'` quando o alvo não existe na máquina; qualquer outro valor é aplicado. */
export async function applyToOwnServers(servers: readonly Server[],
  fn: (server: Server) => Promise<unknown>): Promise<ApplyResult[]> {
  return Promise.all(ownUpdatableServers(servers).map(async (server): Promise<ApplyResult> => {
    try {
      return { serverId: server.id, status: (await fn(server)) === 'absent' ? 'absent' : 'ok' };
    } catch (e) {
      const status = (e as { status?: unknown } | null)?.status;
      const error = e instanceof Error ? e.message : String(e);
      // Sem status HTTP o pedido não chegou a uma resposta: rede, prazo ou servidor esfriado.
      if (typeof status !== 'number') return { serverId: server.id, status: 'offline', error };
      return { serverId: server.id, status: status === 404 ? 'old' : 'failed', error };
    }
  }));
}

/** Contexto de 1M do Codex numa máquina; sem o Codex instalado, `'absent'`. */
export async function applyCodexContext(server: Server, value: boolean): Promise<'absent' | void> {
  const signal = new AbortController().signal;
  const harnesses = await getHarnessesForServer(server, signal);
  if (!harnesses.some((h) => h.id === 'codex' && h.instalado)) return 'absent';
  await codexOpcoes(server, signal, value);
}
