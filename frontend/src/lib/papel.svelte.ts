// Papel desta tela em cada servidor. Backend sem /api/me (versão antiga) responde 404 e conta como dono.
import { getMeForServer } from '@hangar/core';
import * as m from '../paraglide/messages';
import type { Server } from './auth';

/** Rota do dono negada ao convidado: a máquina respondeu. O código já chega traduzido na mensagem,
 *  e só o status confundiria com o 403 de um proxy, que é queda de verdade. */
export function negadaAoConvidado(e: unknown): boolean {
  return e instanceof Error && (e as { status?: number }).status === 403 && e.message.endsWith(m.erro_fora_do_convidado());
}

const roles = $state<Record<string, 'owner' | 'guest'>>({});
const pending = new Set<string>();

export function papelDo(server: Server | null): 'owner' | 'guest' | null {
  if (!server) return null;
  const key = `${server.id}|${server.token}`;
  if (!(key in roles) && !pending.has(key)) {
    pending.add(key);
    getMeForServer(server)
      .then((me) => { roles[key] = me.role; })
      .catch((e: { status?: number }) => {
        // Só resposta HTTP define o papel; falha de rede não grava nada e a próxima leitura tenta de novo.
        if (e?.status === 404) roles[key] = 'owner';
      })
      .finally(() => pending.delete(key));
  }
  return roles[key] ?? null;
}
