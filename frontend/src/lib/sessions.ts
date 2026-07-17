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

// `hidden` = exclusão OTIMISTA: chaves `serverId::name` marcadas pelo doDelete das views somem da
// lista na hora, sem esperar o SSE re-emitir (~1-2s). Se o delete falhar, a view desmarca e a linha
// REAPARECE — rollback visual que o otimismo antigo (filter local no mobile) nunca teve.
export function aggregateSessions(
  servers: Server[],
  slots: ReadonlyMap<string, Slot>,
  hidden?: ReadonlySet<string>,
): Aggregate {
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
        if (hidden?.has(`${srv.id}::${s.name}`)) continue;
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

// Faxina do `hidden`: quando o SSE re-emite a lista do servidor SEM a sessão, o backend já
// confirmou a exclusão — a marca otimista não é mais necessária. Devolve só as chaves ainda
// pendentes (sessão presente na última lista boa, ou servidor sem lista — aí não dá pra saber
// e a marca fica). Puro, pra ser testável no vitest node.
export function sweepHidden(hidden: ReadonlySet<string>, slots: ReadonlyMap<string, Slot>): Set<string> {
  const kept = new Set<string>();
  for (const key of hidden) {
    const i = key.indexOf('::');
    const serverId = key.slice(0, i);
    const name = key.slice(i + 2);
    const sessions = slots.get(serverId)?.sessions;
    if (!sessions || sessions.some((s) => s.name === name)) kept.add(key);
  }
  return kept;
}
