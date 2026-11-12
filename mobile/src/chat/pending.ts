import type { ChatEvent } from '@hangar/core';

// Eco local pendente — o que foi mandado mas o transcript ainda não confirmou.
export interface PendingMsg {
  id: string;
  text: string;
  solid?: boolean;
  /** Último `user_msg` visto no envio: o trecho de várias linhas só confirma em mensagem que chegou depois dele. */
  afterId?: string;
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

  // Também casa pela legenda do pending quando o incoming veio sem marcador mas o pending tinha:
  // ex. pending "foto — 📎 imagem: /tmp/x.jpg" vs incoming "foto"
  return pending.filter((p) => {
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
