import { useCallback, useEffect, useState } from 'react';
import { useFocusEffect } from 'expo-router';
import { getArrivedNews, getManual, getNewsHistory, intlLocale, registrarDiag, type Manual, type NewsHistory, type Server, type UpdateNewsPlatforms } from '@hangar/core';
import { prefs } from '../../stores/prefs';
import { markShownNews, markSeen, updateDeviceId } from './pending';
import * as m from '../../paraglide/messages';

/** O celular mostra o que é dele e o que vale para todos, como a folha do que chegou. */
export const forPhone = (p: UpdateNewsPlatforms) => p === 'mobile' || p === 'all';

export type DocState<T> =
  | { status: 'loading' }
  | { status: 'outdated' }
  | { status: 'error'; message: string }
  | { status: 'ok'; data: T };

type Kind = 'news' | 'manual';
// Em memória: a página da novidade e a do artigo leem o que a lista já trouxe, sem outra ida.
const memory: { news: Map<string, NewsHistory>; manual: Map<string, Manual> } = { news: new Map(), manual: new Map() };
const manualKey = (serverId: string) => `manual.cache.v1:${serverId}`;

// O manual guardado no aparelho abre sem rede; o histórico só fica na memória.
function readManualDisk(serverId: string): Manual | undefined {
  try {
    const raw = prefs.getString(manualKey(serverId));
    const value: unknown = raw ? JSON.parse(raw) : null;
    return value && Array.isArray((value as Manual).categories) ? value as Manual : undefined;
  } catch { return undefined; }
}

function cached<T>(kind: Kind, serverId: string): T | undefined {
  const hit = memory[kind].get(serverId) ?? (kind === 'manual' ? readManualDisk(serverId) : undefined);
  return hit as T | undefined;
}

function store(kind: Kind, serverId: string, data: NewsHistory | Manual) {
  if (kind === 'news') { memory.news.set(serverId, data as NewsHistory); return; }
  memory.manual.set(serverId, data as Manual);
  try { prefs.set(manualKey(serverId), JSON.stringify(data)); } catch { /* só não abre sem rede */ }
}

function errorText(e: unknown): string {
  const { status, code } = (e ?? {}) as { status?: number; code?: string };
  if (status === 403) return m.updates_history_forbidden();
  if (code === 'news_unavailable' || code === 'manual_unavailable') return m.updates_history_unavailable();
  return e instanceof Error && e.message ? e.message : m.erro_desconhecido();
}

// `refresh`: a lista relê sempre; a página de um item só busca quando a memória não tem.
function useDoc<T extends NewsHistory | Manual>(server: Server | null, kind: Kind, refresh: boolean): [DocState<T>, () => void] {
  const [state, setState] = useState<DocState<T>>(() => {
    const hit = server ? cached<T>(kind, server.id) : undefined;
    return hit ? { status: 'ok', data: hit } : { status: 'loading' };
  });
  const [attempt, setAttempt] = useState(0);
  useEffect(() => {
    if (!server) return;
    const hit = cached<T>(kind, server.id);
    setState(hit ? { status: 'ok', data: hit } : { status: 'loading' });
    if (hit && !refresh) return;
    let alive = true;
    const load = (kind === 'news' ? getNewsHistory(server) : getManual(server)) as Promise<T | null>;
    load.then((data) => {
      if (!alive) return;
      if (data === null) { setState({ status: 'outdated' }); return; }
      store(kind, server.id, data);
      setState({ status: 'ok', data });
    }).catch((e: unknown) => {
      // Com o guardado na tela, a falha da releitura não apaga o que a pessoa está lendo.
      if (alive && !hit) setState({ status: 'error', message: errorText(e) });
    });
    return () => { alive = false; };
  }, [server?.id, server?.baseUrl, server?.token, kind, refresh, attempt]);
  const retry = useCallback(() => setAttempt((n) => n + 1), []);
  return [server ? state : { status: 'error', message: m.updates_history_no_server() }, retry];
}

export const useNewsHistory = (server: Server | null, refresh: boolean) => useDoc<NewsHistory>(server, 'news', refresh);
export const useManual = (server: Server | null, refresh: boolean) => useDoc<Manual>(server, 'manual', refresh);

// Até onde esta abertura do app já marcou visto: a contagem de Configurações não espera o POST voltar.
const seenThrough = new Map<string, number>();

/** Ids que chegaram e o aparelho não tinha visto; abrir a tela marca tudo visto, e a bolinha fica até sair. */
export function useUnseenNews(server: Server | null, listShown: boolean): ReadonlySet<string> {
  const [ids, setIds] = useState<ReadonlySet<string>>(() => new Set());
  useEffect(() => {
    // Só marca visto com a lista na tela: com servidor antigo ou erro de leitura, a folha do que chegou continua valendo.
    if (!server || !listShown) return;
    let alive = true;
    getArrivedNews(server, updateDeviceId()).then((arrived) => {
      if (!alive || !arrived || !Number.isSafeInteger(arrived.through)) return;
      setIds(new Set(arrived.news.map((n) => n.id)));
      // A folha do que chegou, se estava esperando, não reaparece com o que a lista já mostrou.
      markShownNews(server.id);
      seenThrough.set(server.id, Math.max(seenThrough.get(server.id) ?? 0, arrived.through));
      markSeen(server, arrived.through);
    }).catch((e: unknown) => {
      registrarDiag({ evento: 'update.arrived_failed', nivel: 'aviso', codigo: 'update_arrived_failed', detalhe: e instanceof Error ? e.message : String(e) });
    });
    return () => { alive = false; };
  }, [server?.id, server?.baseUrl, server?.token, listShown]);
  return ids;
}

/** Entradas de Configurações: escondidas para convidado (403) e sem servidor; `count` = chegadas não vistas. */
export function useDocsEntry(server: Server | null): { visible: boolean; count: number } {
  const [probe, setProbe] = useState<{ id: string; forbidden: boolean; count: number } | null>(null);
  useFocusEffect(useCallback(() => {
    if (!server || server.invite) return;
    let alive = true;
    getArrivedNews(server, updateDeviceId()).then((arrived) => {
      if (!alive) return;
      const seen = arrived && (seenThrough.get(server.id) ?? -1) >= arrived.through;
      const count = arrived && !seen ? arrived.news.filter((n) => forPhone(n.platforms)).length : 0;
      setProbe({ id: server.id, forbidden: false, count });
    }).catch((e: unknown) => {
      if (alive) setProbe({ id: server.id, forbidden: (e as { status?: number }).status === 403, count: 0 });
    });
    return () => { alive = false; };
  }, [server?.id, server?.baseUrl, server?.token, server?.invite]));
  if (!server || server.invite) return { visible: false, count: 0 };
  const mine = probe?.id === server.id ? probe : null;
  return { visible: !mine?.forbidden, count: mine?.count ?? 0 };
}

const parseDay = (date: string) => {
  const [y, mo, d] = date.split('-').map(Number);
  return new Date(y, mo - 1, d);
};

/** "12 de novembro · quinta-feira"; com `year`, o ano entra depois do mês. */
export function dayLabel(date: string, year = false): string {
  const day = parseDay(date);
  if (Number.isNaN(day.getTime())) return date;
  const locale = intlLocale();
  const main = day.toLocaleDateString(locale, { day: 'numeric', month: 'long', ...(year ? { year: 'numeric' } : {}) });
  return `${main} · ${day.toLocaleDateString(locale, { weekday: 'long' })}`;
}

export const shortDay = (date: string) => {
  const day = parseDay(date);
  return Number.isNaN(day.getTime()) ? date : day.toLocaleDateString(intlLocale(), { day: 'numeric', month: 'long' });
};
