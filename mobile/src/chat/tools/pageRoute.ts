import type { HtmlPageRef } from '@hangar/core';

// Tela cheia da página: só o que a tela precisa para buscar o documento e mostrar o título.
export function pageScreenHref(serverId: string, sessionName: string, page: HtmlPageRef): string {
  const q = new URLSearchParams({ id: page.id, title: page.title });
  if (page.ownTheme) q.set('own', '1');
  // Segmentos codificados: nome com `/`, `?` ou `#` quebraria a rota; o expo-router entrega os parâmetros já decodificados.
  return `/s/${encodeURIComponent(serverId)}/${encodeURIComponent(sessionName)}/page?${q.toString()}`;
}

// Parâmetro de rota do expo-router: repetido vira lista, e vale o primeiro.
export const one = (v: string | string[] | undefined) => (Array.isArray(v) ? v[0] : v) ?? '';

// Porta da rota do túnel: o número vem de link, então só inteiro de 1 a 65535.
export function portFromParam(v: string | string[] | undefined): number | null {
  const n = Number(one(v));
  return Number.isInteger(n) && n > 0 && n < 65536 ? n : null;
}

export function pageFromParams(p: { id?: string | string[]; title?: string | string[]; own?: string | string[] }): HtmlPageRef | null {
  const id = one(p.id);
  if (!id) return null;
  return { id, title: one(p.title) || id, height: null, heights: {}, ownTheme: one(p.own) === '1' };
}
