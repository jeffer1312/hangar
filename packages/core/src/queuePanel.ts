// Painel da fila: as mensagens que esperam a vez no Hangar, acima do composer. A lista vem do evento `queue` do
// servidor (Claude e Codex, com ações); sem ele (Pi, omp, Kimi, modo python) sai das bolhas `queued-`, sem ações.
import { waitsInQueue } from './history';
import { composeWithContextRefs, type ContextRef } from './contextRefs';
import type { ChatEvent } from './types';

export interface QueuePanelEntry { id: string; text: string; ts: number | null }
export interface QueuePanel { entries: QueuePanelEntry[]; actions: boolean }
/** Edição em curso: `savedDraft` é o rascunho que estava no composer e volta ao sair. */
export interface QueueEdit { entryId: string; original: string; savedDraft: string }

const PREFIX = 'queued-';

/** `problema` da sessão sem terminal viva e surda: a fila não anda e o painel oferece Reabrir. */
export const DELIVERY_STALLED = 'headless_delivery_stalled';

/** Lê o corpo do evento `queue`; linha torta sai da lista, forma torta devolve `null` (sem painel do servidor). */
export function parseQueuePanel(data: unknown): QueuePanel | null {
  if (!data || typeof data !== 'object') return null;
  const raw = data as { entries?: unknown; actions?: unknown };
  if (!Array.isArray(raw.entries)) return null;
  const entries = raw.entries.flatMap((e): QueuePanelEntry[] => {
    if (!e || typeof e !== 'object') return [];
    const { id, text, ts } = e as { id?: unknown; text?: unknown; ts?: unknown };
    if (typeof id !== 'string' || !id || typeof text !== 'string') return [];
    return [{ id, text, ts: typeof ts === 'number' ? ts : null }];
  });
  return { entries, actions: raw.actions === true };
}

/** Bolhas `queued-<id>` das entradas apagadas noutro aparelho (`removed` do evento `queue`): saem da conversa aqui também. */
export function queueRemovedBubbleIds(data: unknown): string[] {
  const removed = data && typeof data === 'object' ? (data as { removed?: unknown }).removed : undefined;
  if (!Array.isArray(removed)) return [];
  return removed.filter((id): id is string => typeof id === 'string' && id !== '').map((id) => PREFIX + id);
}

/** Sem o evento do servidor: as bolhas `queued-` que ainda esperam, só leitura. */
export function derivedQueuePanel(events: ChatEvent[]): QueuePanel {
  const entries = events
    .filter((e) => waitsInQueue(e) && e.queued_delivered === false && !e.queued_confirmed)
    .map((e) => ({ id: e.id.slice(PREFIX.length), text: e.text ?? '', ts: e.queued_ts ?? null }));
  return { entries, actions: false };
}

export function currentQueuePanel(server: QueuePanel | null, events: ChatEvent[]): QueuePanel {
  return server ?? derivedQueuePanel(events);
}

/** A bolha `queued-` some da conversa enquanto a mensagem está no painel. */
export function hiddenByQueuePanel(ev: ChatEvent, panel: QueuePanel): boolean {
  return ev.kind === 'user_msg' && ev.id.startsWith(PREFIX) && panel.entries.some((e) => PREFIX + e.id === ev.id);
}

export function beginQueueEdit(entry: QueuePanelEntry, draft: string, current: QueueEdit | null): { edit: QueueEdit; composerText: string } {
  // Trocar a linha em edição não troca o rascunho guardado: ele é o de antes da primeira edição.
  return { edit: { entryId: entry.id, original: entry.text, savedDraft: current ? current.savedDraft : draft }, composerText: entry.text };
}

/** O servidor recusa texto vazio e comando (422 `queue_text_invalid`): o botão Atualizar fica desativado antes.
 *  `refs`: citações da mensagem em edição, que voltam na frente do texto; com elas o campo pode ficar vazio. */
export function canUpdateQueueEdit(text: string, refs: readonly ContextRef[] = []): boolean {
  const trimmed = composeWithContextRefs(refs, text);
  return trimmed.length > 0 && !isSlashCommand(trimmed);
}

/** O servidor trata como comando todo texto que começa com `/`, até caminho colado: só no Codex ele entra na fila. */
export function isSlashCommand(text: string): boolean {
  return text.trimStart().startsWith('/');
}

/** O texto que o Atualizar grava (sem as bordas em branco), ou `null` quando ele não mudou e só sai da edição. */
export function queueEditPayload(text: string, current: string | undefined): string | null {
  const next = text.trim();
  return current !== undefined && current.trim() === next ? null : next;
}

/** O eco local de um envio (`pending`) que já está no painel: some, senão a mesma mensagem aparece duas vezes. */
export function echoInQueuePanel(text: string, panel: QueuePanel): boolean {
  const t = text.trim();
  return t !== '' && panel.entries.some((e) => e.text.trim() === t);
}

/** O texto do composer ao sair da edição sem gravar: com o rascunho guardado vazio, a edição fica; senão ele volta. */
export function queueEditExitText(edit: QueueEdit, composerText: string): string {
  return edit.savedDraft.trim() === '' ? composerText : edit.savedDraft;
}

/** A mensagem saiu da fila durante a edição: com o rascunho vazio a edição fica no composer; senão o rascunho volta. */
export function queueEditLeft(edit: QueueEdit, panel: QueuePanel, composerText: string): { composerText: string } | null {
  if (panel.entries.some((e) => e.id === edit.entryId)) return null;
  return { composerText: queueEditExitText(edit, composerText) };
}

/** `before_id` da rota para mover uma posição; `null` é o fim; `undefined`, nada a mover. */
export function moveTarget(entries: QueuePanelEntry[], id: string, delta: -1 | 1): string | null | undefined {
  const neighbor = entries[entries.findIndex((e) => e.id === id) + delta];
  return neighbor ? dropTarget(entries, id, neighbor.id, delta > 0) : undefined;
}

/** `before_id` para soltar `dragId` antes (ou depois, com `after`) de `overId`; `undefined` se a posição não muda. */
export function dropTarget(entries: QueuePanelEntry[], dragId: string, overId: string, after: boolean): string | null | undefined {
  const over = entries.findIndex((e) => e.id === overId);
  const from = entries.findIndex((e) => e.id === dragId);
  if (over < 0 || from < 0) return undefined;
  const before = after ? (entries[over + 1]?.id ?? null) : overId;
  const next = entries[from + 1]?.id ?? null;
  return before === dragId || before === next ? undefined : before;
}

