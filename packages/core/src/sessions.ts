// Agregação multi-servidor da lista de sessões — o CORAÇÃO puro do que era o trio
// slots/recompute/connect copiado em Sidebar/SessionList/Board (docs/polish-backlog.md).
// Puro de propósito: o vitest roda em env node sem plugin svelte, então o que é testável
// mora aqui; o ciclo de vida dos EventSources mora no sessionsStore.svelte.ts.
import type { SessionInfo, AggSession } from './types';
import type { Server } from './servers';
import { serverColor } from './servers';
import { machineLabel, tagHex, type TagsEvent } from './tags';
import { isOwnMachine } from './machineImport';

export type KeyringState = 'unlocked' | 'locked' | 'absent';

// `tags`: último evento `tags` do servidor; ausente = versão antiga, valem o nome e a cor locais.
// `tagsRaw`: o `data` cru dele, para a reconexão que repete o mesmo evento não refazer as linhas.
// `offlineSince`: primeira falha da queda atual; vale até a próxima lista boa (`error: null`).
export interface Slot { sessions: SessionInfo[] | null; error: string | null; keyring?: KeyringState | null; tags?: TagsEvent | null; tagsRaw?: string; offlineSince?: number }

/** Slot novo com o `patch` por cima do anterior: o que ele não cita (etiquetas, chaveiro) segue igual.
 *  `patch.offlineSince` é o início gravado da queda (`startOutage`) e vence o que o slot tem em memória. */
export function patchSlot(slots: ReadonlyMap<string, Slot>, id: string, patch: Partial<Slot>): Slot {
  const prev = slots.get(id);
  const { offlineSince: _, ...next } = { sessions: null, error: null, ...prev, ...patch };
  if (!next.error) return next;
  // Tentativas frustradas regravam o slot, às vezes alternando offline e erro HTTP; a queda
  // continua contando da primeira até uma lista boa.
  const since = patch.offlineSince ?? ((prev?.error && prev.offlineSince) || (next.error === 'offline' ? Date.now() : undefined));
  return since ? { ...next, offlineSince: since } : next;
}

// A máquina pode só estar desligada: o aviso vermelho cobre as primeiras tentativas, não a queda inteira.
export const OFFLINE_NOTICE_MS = 30_000;

type Notice = { error: string | null; offlineSince?: number };

/** Até quando o aviso da máquina fica na tela: 0 sem aviso; erro que não é queda não expira. */
function noticeUntil(b: Notice): number {
  if (!b.error) return 0;
  if (b.error !== 'offline') return Infinity;
  return b.offlineSince ? b.offlineSince + OFFLINE_NOTICE_MS : 0;
}

export function machineNoticeVisible(b: Notice, now: number): boolean {
  return noticeUntil(b) > now;
}

/** Toda máquina própria ligada fora do ar: lista vazia não é "nenhuma sessão ativa". */
export function machinesAllDown(buckets: readonly { server: Pick<Server, 'invite' | 'ephemeral' | 'removed' | 'disabled'>; error: string | null }[]): boolean {
  const own = buckets.filter((b) => isOwnMachine(b.server) && !b.server.disabled);
  return own.length > 0 && own.every((b) => b.error === 'offline');
}

/** Próximo instante em que um aviso some, para um único timer redesenhar a tela; null = nenhum. */
export function nextNoticeEnd(buckets: readonly Notice[], now: number): number | null {
  let next: number | null = null;
  for (const b of buckets) {
    const until = noticeUntil(b);
    if (until > now && Number.isFinite(until) && (next === null || until < next)) next = until;
  }
  return next;
}

export interface ServerBucket {
  server: Server;
  // Nome e cor da máquina já resolvidos pela etiqueta; ausentes = valem `server.label` e `serverColor`.
  label?: string;
  color?: string;
  sessions: AggSession[];   // já deduplicadas e enriquecidas
  error: string | null;     // 'offline' quando o stream caiu ou veio frame malformado
  loaded: boolean;          // já recebeu ao menos uma lista boa (pode coexistir com error = stale)
  keyringLocked?: boolean;  // evento `keyring` da lista: o chaveiro `login` da máquina está trancado
  offlineSince?: number;    // primeira falha da queda atual (ver `machineNoticeVisible`)
}

export interface Aggregate {
  rows: AggSession[];        // achatado, dedup global, na ordem de `servers`
  byServer: ServerBucket[];  // 1 bucket por servidor, SEMPRE presente (mesmo vazio/offline)
  loading: boolean;          // nenhum servidor emitiu nada ainda (nem erro)
}

// Memo de enriquecimento por SLOT (o array `sessions` cru de um servidor): um evento SSE troca o
// array só do servidor que emitiu, então as rows dos DEMAIS servidores são reusadas por identidade.
// Sem isto, todo evento recriava {...s} pra TODAS as rows -> o keyed each das 4 views via prop nova
// em 50+ cards e re-renderizava tudo ~3-4x/s com 5 servidores ativos. WeakMap: o array velho morre,
// a entrada vai junto. label/color validados porque o usuário pode editar o servidor sem o slot mudar.
const _rowCache = new WeakMap<object, { label: string; color: string; invite: boolean; tags: TagsEvent | null; byKey: Map<string, AggSession> }>();

// `hidden` = exclusão OTIMISTA: chaves `serverId::name` marcadas pelo doDelete das views somem da
// lista na hora, sem esperar o SSE re-emitir (~1-2s). Se o delete falhar, a view desmarca e a linha
// REAPARECE — rollback visual que o otimismo antigo (filter local no mobile) nunca teve.
// (O filtro roda ANTES do memo: esconder/mostrar não invalida as rows cacheadas do slot.)
export function aggregateSessions(
  servers: Server[],
  slots: ReadonlyMap<string, Slot>,
  hidden?: { has(key: string): boolean },
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
      keyringLocked: slot?.keyring === 'locked' && slot.error !== 'offline',
      offlineSince: slot?.offlineSince,
    };
    if (slot?.sessions && slot.error !== 'offline') {
      const tags = slot.tags ?? null;
      const color = tagHex(tags?.machine?.color) ?? serverColor(srv.id);
      const label = machineLabel(srv, tags);
      bucket.label = label;
      bucket.color = color;
      const invite = srv.invite === true;
      let cache = _rowCache.get(slot.sessions);
      if (!cache || cache.label !== label || cache.color !== color || cache.invite !== invite || cache.tags !== tags) {
        cache = { label, color, invite, tags, byKey: new Map() };
        _rowCache.set(slot.sessions, cache);
      }
      for (const s of slot.sessions) {
        if (hidden?.has(`${srv.id}::${s.name}`)) continue;
        const key = `${s.jsonl ?? s.cwd ?? ''}::${s.name}`;
        if (seen.has(key)) continue;
        seen.add(key);
        let row = cache.byKey.get(key);
        if (!row) {
          row = { ...s, serverId: srv.id, serverLabel: label, serverColor: color, serverInvite: invite,
            machineTag: tags?.machine ?? null, accountTag: (s.conta && tags?.accounts[s.conta]) || null };
          cache.byKey.set(key, row);
        }
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
// Época de RECRIAÇÃO por `serverId::name`: sobe quando uma sessão some da lista e outra com o mesmo
// nome aparece depois. O Chat do desktop é remontado por `{#key}` com o nome — matar `x` e criar
// outra `x` dava a mesma chave, e a conversa da morta ficava na tela enquanto a nova subia (pré-thread
// do Codex). Lê os `slots` e não a lista filtrada: a exclusão otimista (`hidden`) e um servidor
// offline (que guarda a última lista) não são sumiço.
// Quem separa recriação de sumiço passageiro é o TRANSCRIPT: a sessão nova nasce com outro jsonl
// (ou nenhum), a mesma volta com o de antes. Só a ausência do nome remontava o Chat — e com isso
// perdia foco, rolagem e prévia — em qualquer lista que deixasse de citar uma sessão viva por um
// instante.
export interface Epocas {
  vistas: ReadonlyMap<string, string | null>;    // chave -> jsonl na última lista
  sumidas: ReadonlyMap<string, string | null>;   // chave -> jsonl com que sumiu
  epochs: ReadonlyMap<string, number>;
}

export function epocasDeRecriacao(anterior: Epocas, slots: ReadonlyMap<string, Slot>): Epocas {
  const atuais = new Map<string, string | null>();
  for (const [serverId, slot] of slots) for (const s of slot.sessions ?? []) atuais.set(`${serverId}::${s.name}`, s.jsonl ?? null);
  const sumidas = new Map(anterior.sumidas);
  for (const [k, jsonl] of anterior.vistas) if (!atuais.has(k)) sumidas.set(k, jsonl);
  let epochs = anterior.epochs;
  for (const [k, jsonl] of atuais) {
    if (!sumidas.has(k)) continue;
    const antes = sumidas.get(k) ?? null;
    sumidas.delete(k);
    if (antes === jsonl) continue;
    if (epochs === anterior.epochs) epochs = new Map(epochs);
    (epochs as Map<string, number>).set(k, (epochs.get(k) ?? 0) + 1);
  }
  return { vistas: atuais, sumidas, epochs };
}

// A marca guarda o `jsonl` da sessão que está sendo excluída: uma sessão NOVA com o mesmo nome
// (jsonl outro, ou nenhum ainda) não é a que morreu, e não pode herdar a marca — senão a recriada
// ficava escondida da lista até alguém a fechar de novo. Sem lista do servidor não dá pra saber.
export function jsonlDaSessao(slots: ReadonlyMap<string, Slot>, serverId: string, name: string): string | null {
  return slots.get(serverId)?.sessions?.find((s) => s.name === name)?.jsonl ?? null;
}

export function sweepHidden(hidden: ReadonlyMap<string, string | null>,
                            slots: ReadonlyMap<string, Slot>): Map<string, string | null> {
  const kept = new Map<string, string | null>();
  for (const [key, jsonl] of hidden) {
    const i = key.indexOf('::');
    const serverId = key.slice(0, i);
    const name = key.slice(i + 2);
    const sessions = slots.get(serverId)?.sessions;
    if (!sessions || sessions.some((s) => s.name === name && (s.jsonl ?? null) === jsonl)) kept.set(key, jsonl);
  }
  return kept;
}
