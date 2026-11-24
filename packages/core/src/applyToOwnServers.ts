// "Em todas as minhas máquinas": repete uma gravação em cada servidor próprio e diz, por máquina,
// o que não foi aplicado e por quê. Sem fila durável: quem não respondeu é tentado de novo pela tela.
import type { Server } from './servers';
import { ownUpdatableServers } from './updateAll';
import { codexOpcoes, getEnginesForServer, getHarnessesForServer, saveSessionDefaultsForServer, setClaudeDefaultsForServer,
  type CliProxyAccount, type SessionDefaults } from './api';
import type { Provider } from './types';

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

/** O padrão de criação a repetir: escopo, valor (`null` apaga) e se o Claude sem motor também vai ao settings.json. */
export interface SessionDefaultWrite {
  provider: Provider; engine?: string | null; engineAccount?: string | null; value: SessionDefaults | null; mirror: boolean;
}

// Conta do proxy só serve com id e credencial do Codex ou do CLIProxyAPI, como no app de PC.
const eligibleAccount = (a: CliProxyAccount | undefined) => !!a?.account && /^(codex|cliproxy):./.test(a.credential_id);

/** Grava o padrão de criação numa máquina; motor ou conta dele que não existe lá dá `'absent'`. */
export async function applySessionDefault(server: Server, write: SessionDefaultWrite): Promise<'absent' | void> {
  if (write.engine) {
    const motor = (await getEnginesForServer(server)).motores[write.engine];
    if (!motor) return 'absent';
    const account = write.engineAccount;
    // Id de conta é local: gravar sem ela lá deixaria um padrão que nunca vale.
    if (account && (motor.cliproxy_error || !motor.cliproxy_accounts || !(motor.cliproxy_binding
      ? motor.cliproxy_bound_account?.account === account && eligibleAccount(motor.cliproxy_bound_account)
      : motor.cliproxy_accounts.some((a) => a.account === account && eligibleAccount(a))))) return 'absent';
  }
  await saveSessionDefaultsForServer(server, write.provider, write.value, write.engine, write.engineAccount);
  if (write.value && write.mirror) await setClaudeDefaultsForServer(server, write.value);
}
