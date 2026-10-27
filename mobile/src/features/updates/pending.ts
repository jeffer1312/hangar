import type { PreparedUpdateAll, Server, UpdateAcceptance, UpdateNews } from '@hangar/core';
import { prefs } from '../../stores/prefs';

// O que a folha "Atualizar?" mostrou, guardado por servidor até "O que chegou para você" exibir uma vez.
const PREFIX = 'update.whatsNew.v1:';
const key = (serverId: string) => PREFIX + serverId;

// `from`: versão do servidor ao confirmar; na abertura do app, versão igual = a atualização não terminou.
export type PendingNews = { cards: UpdateNews[]; fixes: UpdateNews[]; from: string };

export function savePendingNews(serverId: string, news: Pick<PendingNews, 'cards' | 'fixes'>, from = ''): void {
  try { prefs.set(key(serverId), JSON.stringify({ cards: news.cards, fixes: news.fixes, from } satisfies PendingNews)); }
  catch { /* sem a lista guardada, a tela de depois só não aparece */ }
}

const EMPTY: PendingNews = { cards: [], fixes: [], from: '' };

/** `null` = nada guardado; ilegível vira lista vazia, para quem lê apagar. */
export function readPendingNews(serverId: string): PendingNews | null {
  try {
    const raw = prefs.getString(key(serverId));
    if (!raw) return null;
    const value = JSON.parse(raw) as Partial<PendingNews> | null;
    if (!Array.isArray(value?.cards) || !Array.isArray(value.fixes)) return EMPTY;
    return { cards: value.cards, fixes: value.fixes, from: typeof value.from === 'string' ? value.from : '' };
  } catch {
    return EMPTY;
  }
}

export const pendingFrom = (serverId: string): string => readPendingNews(serverId)?.from ?? '';

export function clearPendingNews(serverId: string): void {
  try { prefs.remove(key(serverId)); } catch { /* nada a fazer */ }
}

/** Primeiro servidor da lista com novidades guardadas; lista vazia ou ilegível é apagada no caminho. */
export function firstPendingServer(serverIds: readonly string[]): string | null {
  for (const id of serverIds) {
    const pending = readPendingNews(id);
    if (pending === null) continue;
    if (pending.cards.length || pending.fixes.length) return id;
    clearPendingNews(id);
  }
  return null;
}

/** Aceite dos avisos lidos, por id de servidor; vazio quando nenhum aviso apareceu. */
export type UpdateAcceptances = Record<string, UpdateAcceptance>;

// A rota da folha não recebe callback: quem pede confirmação deixa o pedido aqui e espera a resposta.
// `machines`: no "Atualizar todas", os avisos de cada máquina, lidos por quem pediu.
export type ConfirmRequest = { server: Server; count?: number; machines?: Promise<PreparedUpdateAll>;
  resolve: (ok: boolean, accept?: UpdateAcceptances) => void };
let request: ConfirmRequest | null = null;

/** `null` = cancelou. */
export function requestUpdateConfirm(server: Server, open: () => void, count?: number, machines?: Promise<PreparedUpdateAll>): Promise<UpdateAcceptances | null> {
  request?.resolve(false);
  return new Promise((resolve) => {
    let done = false;
    request = { server, count, machines, resolve: (ok, accept = {}) => {
      if (done) return; done = true; request = null; resolve(ok ? accept : null);
    } };
    open();
  });
}

export const currentConfirmRequest = () => request;

export const whatsNewHref = (serverId: string) => ({ pathname: '/whats-new', params: { server: serverId } }) as never;
