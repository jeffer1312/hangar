// Modelo puro das abas horizontais (Task 6, item A do backlog): a sidebar recolhida vira abas no
// topo. Ordem: buckets na ordem de `servers`, e `sortSessions` dentro de cada bucket (mesma ordem
// das listas). Bucket com erro/offline não gera aba acionável — só o rótulo do servidor entra no
// indicador `⚠ N offline`. Testável no vitest node (sem svelte).
import { sortSessions } from './format';
import type { AggSession } from './types';
import type { ServerBucket } from './sessions';

export interface SessionTab { session: AggSession; boundary: boolean }
export interface SessionTabsModel { tabs: SessionTab[]; offlineLabels: string[] }

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
