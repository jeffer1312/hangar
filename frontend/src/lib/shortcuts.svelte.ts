// DONO da lista de atalhos da fileira, por servidor. Mora fora do Chat porque três superfícies
// consomem a mesma lista (fileira do painel, "⋯" da NavBar, command palette) e o editor da
// config grava nela — uma busca por montagem de Chat viraria três GETs e três verdades.
import {
  defaultShortcuts, getConfig, getConfigForServer, patchConfig, patchConfigForServer,
  resolveShortcuts, serializeShortcuts, type Shortcut,
} from '@hangar/core';
import { getActiveId, listServers } from './auth';

const lists = $state<Record<string, Shortcut[]>>({});
// Promessa, não flag: quem chega com a busca em voo tem de esperar a lista real, senão o editor
// abre com o conjunto nativo e salvar apagaria a config da pessoa.
const inFlight = new Map<string, Promise<void>>();

function keyFor(serverId?: string | null): string {
  return serverId || getActiveId() || '';
}

/** Lista renderizável do servidor (default nativo enquanto não carregou). Pura: quem monta
 * chama `loadShortcuts` num $effect/onMount — side-effect dentro de $derived travaria o Svelte. */
export function shortcutsFor(serverId?: string | null): Shortcut[] {
  return lists[keyFor(serverId)] ?? defaultShortcuts();
}

/** Falha rejeita: enquanto isso a fileira segue no default nativo, e quem chama decide como
 * mostrar o erro. Não cacheia a falha — a próxima chamada tenta de novo. */
export function loadShortcuts(serverId?: string | null): Promise<void> {
  const k = keyFor(serverId);
  if (!k || k in lists) return Promise.resolve();
  const pending = inFlight.get(k);
  if (pending) return pending;
  const load = (async () => {
    const s = serverId && serverId !== getActiveId()
      ? listServers().find((x) => x.id === serverId)
      : null;
    const c = s ? await getConfigForServer(s) : await getConfig();
    lists[k] = resolveShortcuts(String(c.campos.shortcuts?.valor ?? ''));
  })().finally(() => inFlight.delete(k));
  inFlight.set(k, load);
  return load;
}

/** Grava a lista no servidor e atualiza o cache. `null` = remover o override (volta ao nativo). */
export async function saveShortcuts(list: Shortcut[] | null, serverId?: string | null): Promise<void> {
  const k = keyFor(serverId);
  const s = serverId && serverId !== getActiveId()
    ? listServers().find((x) => x.id === serverId)
    : null;
  const value = list === null ? null : serializeShortcuts(list);
  if (s) await patchConfigForServer(s, { shortcuts: value });
  else await patchConfig({ shortcuts: value });
  lists[k] = list === null ? defaultShortcuts() : list;
}
