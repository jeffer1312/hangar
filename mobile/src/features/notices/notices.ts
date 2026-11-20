// Avisos da CLI do Claude na conversa: a mesma leitura do `notice_line.rs` do nativo.
import type { ChatEvent } from '@hangar/core';
import * as m from '../../paraglide/messages';

/** Evento que o servidor marcou como aviso: o mapa presente, mesmo vazio. */
export const isNoticeLine = (ev: ChatEvent) => ev.notice_meta != null;

/** Chave de ignorar; aviso de bloqueio real não tem. */
export const noticeKey = (ev: ChatEvent) => ev.notice_meta?.key ?? null;

/** Resposta que a CLI gravou no lugar da do modelo quando a API falhou: o turno parou. */
export const isApiError = (ev: ChatEvent) => ev.notice_meta?.level === 'error';

/** O texto sem o `⚙️` que o servidor põe para o cliente antigo. */
export function plainNotice(ev: ChatEvent): string {
  const text = ev.text ?? '';
  return text.startsWith('⚙️') ? text.slice('⚙️'.length).trimStart() : text;
}

export function noticeTitle(ev: ChatEvent): string {
  const hook = ev.notice_meta?.subject;
  if (hook) return m.notice_hook_failed_line({ name: hook });
  return plainNotice(ev).split('\n').map((line) => line.trim()).find(Boolean) ?? '';
}

/** "12 vezes", só quando o hook falhou mais de uma. */
export function noticeCount(ev: ChatEvent): string | null {
  const count = ev.notice_meta?.count;
  return typeof count === 'number' && count > 1 ? m.notice_count({ count: String(count) }) : null;
}

/** O que abre ao tocar: o detalhe do hook, ou o aviso inteiro quando ele não cabe na linha. */
export function noticeDetail(ev: ChatEvent): string | null {
  const detail = (ev.hook_error ?? plainNotice(ev)).trim();
  return detail && (ev.hook_error != null || detail !== noticeTitle(ev)) ? detail : null;
}

export type NoticeAction = { kind: 'reinstall'; plugin: string } | { kind: 'fix' };

/** O botão que o servidor deu ao aviso. */
export function noticeAction(ev: ChatEvent): NoticeAction | null {
  const meta = ev.notice_meta;
  if (meta?.action === 'reinstall_plugin') return meta.plugin ? { kind: 'reinstall', plugin: meta.plugin } : null;
  return meta?.action === 'fix_hook' ? { kind: 'fix' } : null;
}

/** Nota de conserto: a de andamento muda a linha do aviso e não tem linha própria; a do resultado tem. */
export const isFixNote = (ev: ChatEvent) => ev.notice_meta?.fix_of != null;
export const isFixRunningNote = (ev: ChatEvent) => isFixNote(ev) && ev.notice_meta?.state === 'running';

export type FixState = { state: 'running'; child: string } | { state: 'done' | 'failed' };

type Latest = Map<string, { attempt: number; state: FixState }>;
const RANK = { running: 0, failed: 1, done: 2 } as const;

/** Soma uma nota de conserto ao estado; `true` quando ela mudou alguma coisa. */
function absorb(latest: Latest, ev: ChatEvent): boolean {
  const meta = ev.notice_meta;
  const of = meta?.fix_of;
  const kind = meta?.state;
  if (!meta || !of || (kind !== 'running' && kind !== 'done' && kind !== 'failed')) return false;
  const attempt = meta.attempt ?? 0;
  const now = latest.get(of);
  if (now && !(attempt > now.attempt || (attempt === now.attempt && RANK[kind] > RANK[now.state.state]))) return false;
  latest.set(of, { attempt, state: kind === 'running' ? { state: 'running', child: meta.child ?? '' } : { state: kind } });
  return true;
}

/** Por aviso, a tentativa de maior `attempt`; nela, o resultado vence o andamento. */
export function fixStates(events: readonly ChatEvent[]): Map<string, FixState> {
  return fixTracker()(events);
}

/**
 * O mesmo cálculo para uma lista que cresce: com os eventos de antes iguais, lê só os novos. Devolve o mesmo mapa
 * enquanto nenhum conserto muda, para a lista não redesenhar as linhas. `revision` muda quando a lista troca ou tira
 * eventos no meio: aí relê tudo.
 */
export function fixTracker(): (events: readonly ChatEvent[], revision?: number) => Map<string, FixState> {
  let seen: readonly ChatEvent[] = [];
  let seenRevision: number | undefined;
  let latest: Latest = new Map();
  let out = new Map<string, FixState>();
  return (events, revision) => {
    const last = seen.length - 1;
    const grew = revision === seenRevision && last >= 0 && events.length > last && events[last] === seen[last];
    seenRevision = revision;
    if (!grew) latest = new Map();
    let changed = !grew;
    for (let i = grew ? seen.length : 0; i < events.length; i++) changed = absorb(latest, events[i]) || changed;
    seen = events;
    if (changed) {
      const next = new Map([...latest].map(([of, { state }]) => [of, state]));
      if (!sameFixStates(out, next)) out = next;
    }
    return out;
  };
}

/** Mesmo estado para os mesmos avisos: a lista guarda o mapa anterior e só redesenha as linhas quando um conserto muda. */
export function sameFixStates(a: ReadonlyMap<string, FixState>, b: ReadonlyMap<string, FixState>): boolean {
  if (a.size !== b.size) return false;
  for (const [of, state] of b) {
    const before = a.get(of);
    if (!before || before.state !== state.state || (state.state === 'running' && before.state === 'running' && before.child !== state.child)) return false;
  }
  return true;
}

/** Como uma chave ignorada aparece nas Configurações; a que este app não conhece aparece crua. */
export function noticeKeyLabel(key: string): string {
  if (key.startsWith('hook:')) return m.notice_key_hook({ name: key.slice('hook:'.length) });
  if (key.startsWith('note:plugin:')) return m.notice_key_plugin({ name: key.slice('note:plugin:'.length) });
  if (key.startsWith('note:notification:')) return m.notice_key_notification({ name: key.slice('note:notification:'.length) });
  if (key.startsWith('note:startup:')) return m.notice_key_startup();
  if (key === 'note:retry') return m.notice_key_retry();
  return key;
}

/** `{"keys":[…]}` da rota e do evento; torto é `null`. */
export function parseNoticeIgnores(data: unknown): string[] | null {
  let value = data;
  if (typeof value === 'string') {
    try { value = JSON.parse(value); } catch { return null; }
  }
  const keys = (value as { keys?: unknown } | null)?.keys;
  return Array.isArray(keys) && keys.every((key) => typeof key === 'string') ? keys : null;
}
