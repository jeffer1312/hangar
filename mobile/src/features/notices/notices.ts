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

/** Como uma chave ignorada aparece nas Configurações; a que este app não conhece aparece crua. */
export function noticeKeyLabel(key: string): string {
  if (key.startsWith('hook:')) return m.notice_key_hook({ name: key.slice('hook:'.length) });
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
