import type { UpdateNews, UpdateNotes, UpdateWarning } from './types';

export interface ClientUpdateNotes { warnings: UpdateWarning[]; cards: UpdateNews[]; fixes: UpdateNews[] }

/** O que um cliente mostra: novidades dele ou de todos, cartões separados das correções. */
export function forClient(notes: UpdateNotes, client: 'desktop' | 'mobile'): ClientUpdateNotes {
  const mine = notes.news.filter((n) => n.platforms === client || n.platforms === 'all');
  return {
    warnings: notes.warnings,
    cards: mine.filter((n) => n.kind !== 'fix'),
    // Correção é linha de lista, nunca cartão animado.
    fixes: mine.filter((n) => n.kind === 'fix').map((n) => ({ ...n, animation: null })),
  };
}
