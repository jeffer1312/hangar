// Agregação multi-servidor da lista de sessões — o CORAÇÃO puro do que era o trio
// slots/recompute/connect copiado em Sidebar/SessionList/Board (docs/polish-backlog.md).
// Puro de propósito: o vitest roda em env node sem plugin svelte, então o que é testável
// mora aqui; o ciclo de vida dos EventSources mora no sessionsStore.svelte.ts.
import type { SessionInfo, AggSession } from './types';
import type { Server } from './auth';
import { serverColor } from './auth';

export interface Slot { sessions: SessionInfo[] | null; error: string | null }

export interface ServerBucket {
  server: Server;
  sessions: AggSession[];   // já deduplicadas e enriquecidas
  error: string | null;     // 'offline' quando o stream caiu ou veio frame malformado
  loaded: boolean;          // já recebeu ao menos uma lista boa (pode coexistir com error = stale)
}

export interface Aggregate {
  rows: AggSession[];        // achatado, dedup global, na ordem de `servers`
  byServer: ServerBucket[];  // 1 bucket por servidor, SEMPRE presente (mesmo vazio/offline)
  loading: boolean;          // nenhum servidor emitiu nada ainda (nem erro)
}

export function aggregateSessions(servers: Server[], slots: ReadonlyMap<string, Slot>): Aggregate {
  const seen = new Set<string>(); // dedup global: backend compartilhado por 2 URLs não duplica
  const rows: AggSession[] = [];
  const byServer: ServerBucket[] = [];
  let any = false;
  for (const srv of servers) {
    const slot = slots.get(srv.id);
    if (slot && (slot.sessions !== null || slot.error)) any = true;
    const bucket: ServerBucket = {
      server: srv,
      sessions: [],
      error: slot?.error ?? null,
      loaded: slot?.sessions != null,
    };
    if (slot?.sessions) {
      for (const s of slot.sessions) {
        const key = `${s.jsonl ?? s.cwd ?? ''}::${s.name}`;
        if (seen.has(key)) continue;
        seen.add(key);
        const row: AggSession = { ...s, serverId: srv.id, serverLabel: srv.label, serverColor: serverColor(srv.id) };
        bucket.sessions.push(row);
        rows.push(row);
      }
    }
    byServer.push(bucket);
  }
  return { rows, byServer, loading: servers.length > 0 && !any };
}
