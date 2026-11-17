import type { ChatEvent } from '@hangar/core';

// Eco local pendente — o que foi mandado mas o transcript ainda não confirmou.
export interface PendingMsg {
  id: string;
  text: string;
  solid?: boolean;
  /** Último `user_msg` visto no envio: o trecho de várias linhas só confirma em mensagem que chegou depois dele. */
  afterId?: string;
  /** Enviada com a sessão trabalhando: o lugar dela é o painel "Na fila", não o fim da conversa. */
  queue?: boolean;
  /** Mandada "Na conversa": o eco fica no fim da conversa sem atenuar, como a bolha "Em espera" que o substitui. */
  conversation?: boolean;
  /** As entradas que o painel já listava no envio: nenhuma delas é a deste eco. */
  known?: string[];
  /** A bolha `queued-` do servidor que este eco representa: ela fica escondida enquanto o eco existir. */
  entry?: string;
}

/** Quanto a fila espera para aparecer: a mensagem que já está indo passa por ela sem piscar. */
export const QUEUE_APPEAR_MS = 600;
/** O eco preso a uma entrada da fila sai sozinho depois disto, se a mensagem real não chegou. */
export const ECHO_SETTLE_MS = 5000;

/** A bolha `queued-` chegou: o eco que já está no destino (fim da conversa ou painel) continua sendo a mensagem
 *  na tela e só guarda qual bolha é a dele. Sem eco para ela, devolve a mesma lista. */
export function bindQueued(pending: PendingMsg[], incoming: ChatEvent): PendingMsg[] {
  if (incoming.kind !== 'user_msg' || !incoming.id.startsWith('queued-') || !incoming.text) return pending;
  const id = incoming.id.slice('queued-'.length);
  const at = pending.findIndex((p) => (p.solid || p.queue) && !p.entry && !p.known?.includes(id) && sameMessage(p.text, incoming.text!));
  if (at < 0) return pending;
  return pending.map((p, i) => (i === at ? { ...p, entry: incoming.id } : p));
}

/** O painel do servidor mudou: o eco de quem ia para a fila sai quando a entrada dele aparece lá (a linha do
 *  servidor toma o lugar), e o eco de entrada apagada sai junto. Cada entrada tira um eco só, e nunca uma que já
 *  estava no painel quando ele foi enviado: duas mensagens iguais são duas linhas. */
export function settleQueued(pending: PendingMsg[], listed: readonly { id: string; text: string }[], removed: string[]): PendingMsg[] {
  const taken = new Set<string>();
  const next = pending.filter((p) => {
    if (p.entry && removed.includes(p.entry)) return false;
    if (!p.queue) return true;
    const bound = p.entry ? listed.find((e) => `queued-${e.id}` === p.entry) : undefined;
    if (bound) { taken.add(bound.id); return false; }
    const entry = listed.find((e) => !taken.has(e.id) && !p.known?.includes(e.id) && e.text.trim() === p.text.trim());
    if (!entry) return true;
    taken.add(entry.id);
    return false;
  });
  if (next.length === pending.length) return pending;
  // O próximo retrato repete a entrada que já tirou um eco: os ecos que ficaram passam a conhecê-la.
  return taken.size ? next.map((p) => (p.queue ? { ...p, known: [...(p.known ?? []), ...taken] } : p)) : next;
}

// Legenda canônica: sem o marcador "📎 imagem:/arquivo: `path`" + "—" que liga.
// Cópia da regra do Chat.svelte (_cap) — lá ela casa msg COM anexo vs legenda do transcript.
function cap(text: string): string {
  const i = text.search(/(?:\s*—\s*)?📎\s*(?:imagem|arquivo):/u);
  return (i >= 0 ? text.slice(0, i) : text).trim();
}

// Confirmação de entrega: a mensagem inteira, nunca uma linha de outra.
export function sameMessage(sent: string, incoming: string): boolean {
  const s = sent.trim();
  const t = incoming.trim();
  return s === t || (!!cap(s) && cap(s) === cap(t));
}

// Remove do pending o que o evento recém-chegado já cobre.
// Regra de dedup do Chat.svelte: texto normalizado + legenda + linha.
// - idempotente: chamar duas vezes com o mesmo incoming não muda nada além da 1ª.
// - preserva ordem do pending remanescente.
// `order` (posição de cada evento) vem na reconstrução, que repassa o histórico inteiro; ao vivo o evento é sempre novo.
export function reconcilePending(pending: PendingMsg[], incoming: ChatEvent, order?: Map<string, number>): PendingMsg[] {
  if (incoming.kind !== 'user_msg' || !incoming.text) return pending;
  const t = incoming.text.trim();
  if (!t) return pending;
  const c = cap(t);
  const lines = t.split('\n').map((l) => l.trim()).filter(Boolean);
  // Set de formas commitadas: o texto cru + a legenda + cada linha (mesma semântica do
  // $effect de dedup do Chat.svelte que monta `committed` sobre todos os events).
  // Aqui é incremental: só o incoming desta vez.
  const committed = new Set<string>();
  committed.add(t);
  if (c) committed.add(c);
  for (const ln of lines) committed.add(ln);
  // A bolha de uma entrada que o eco já conhecia é de outra mensagem de mesmo texto: não é ela que o confirma.
  const entry = incoming.id.startsWith('queued-') ? incoming.id.slice('queued-'.length) : null;

  // Também casa pela legenda do pending quando o incoming veio sem marcador mas o pending tinha:
  // ex. pending "foto — 📎 imagem: /tmp/x.jpg" vs incoming "foto"
  return pending.filter((p) => {
    if (entry && p.known?.includes(entry)) return true;
    const pt = p.text.trim();
    if (committed.has(pt)) return false;
    const pc = cap(pt);
    if (pc && committed.has(pc)) return false;
    // linha do pending que casa com linha do incoming (multilinha fundida pelo Claude: "a\nb")
    // O pending pode ser 1 linha que faz parte de um incoming multilinha.
    if (pc && lines.includes(pc)) return false;
    // Mensagens que chegam no meio do turno o Claude grava num `user` só, separadas por quebra de linha.
    const after = p.afterId === undefined ? undefined : order?.get(p.afterId);
    const newer = after === undefined || (order?.get(incoming.id) ?? Infinity) > after;
    if (newer && pt.includes('\n') && `\n${t}\n`.includes(`\n${pt}\n`)) return false;
    return true;
  });
}
