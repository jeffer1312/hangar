// Modelo puro das abas horizontais (Task 6, item A do backlog): a sidebar recolhida vira abas no
// topo. Ordem: buckets na ordem de `servers`, e `sortSessions` dentro de cada bucket (mesma ordem
// das listas). Bucket com erro/offline não gera aba acionável — só o rótulo do servidor entra no
// indicador `⚠ N offline`. Testável no vitest node (sem svelte).
import { sortSessions } from '@hangar/core';
import type { AggSession } from '@hangar/core';
import type { ServerBucket } from './sessions';

export interface SessionTab { session: AggSession; boundary: boolean }
export interface SessionTabsModel { tabs: SessionTab[]; offlineLabels: string[] }

// Chave SERVER-AWARE de uma aba (igual currentKey/workspaceSessionKey): homônimas em servidores
// diferentes têm o MESMO nome — sem o servidor, a chave colide e o tabindex/aria-selected erram.
export const tabKeyOf = (s: AggSession) => `${s.serverId}::${s.name}`;

// Chave focável (roving tabindex): a ÚNICA aba com tabindex=0. Ordem: focusedKey (foco manual via
// Tab/setas) se ainda existe na lista -> currentKey (seleção) se existe -> primeira aba.
// null = sem abas (nada focável). A seleção (aria-selected) e o foco são independentes: trocar a
// sessão selecionada não move o foco — quem focou a aba X continua nela.
export function focusedTabKey(
  tabs: SessionTab[],
  currentKey: string | null,
  focusedKey: string | null,
): string | null {
  if (focusedKey && tabs.some((t) => tabKeyOf(t.session) === focusedKey)) return focusedKey;
  if (currentKey && tabs.some((t) => tabKeyOf(t.session) === currentKey)) return currentKey;
  return tabs[0] ? tabKeyOf(tabs[0].session) : null;
}

export function buildSessionTabs(buckets: ServerBucket[]): SessionTabsModel {
  const tabs: SessionTab[] = [];
  const offlineLabels: string[] = [];
  for (const bucket of buckets) {
    if (bucket.error) {
      offlineLabels.push(bucket.server.label);
      continue;
    }
    const ordered = sortSessions([...bucket.sessions]);
    for (const [index, session] of ordered.entries()) {
      tabs.push({ session, boundary: index === 0 && tabs.length > 0 });
    }
  }
  return { tabs, offlineLabels };
}
