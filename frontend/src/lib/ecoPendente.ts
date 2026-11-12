import { attachmentCaption, type ChatEvent } from '@hangar/core';

export function userMsgs(events: ChatEvent[]): { id: string; text: string }[] {
  return events.flatMap((e) => e.kind === 'user_msg' && e.text ? [{ id: e.id, text: e.text }] : []);
}

/**
 * O eco local sai quando o transcript traz a mensagem: inteira, pela legenda (sem o "📎 imagem: <path>") ou por uma linha.
 * `afterId` é o último `user_msg` do transcript visto no envio: o trecho de várias linhas só confirma em mensagem que chegou depois dele.
 * Ausente da lista, a janela já cortou o começo e tudo o que sobrou é mais novo.
 */
export function ecoEntregue(userMsgs: { id: string; text: string }[]): (sent: string, afterId?: string) => boolean {
  const forms = new Set<string>();
  const blocks: { at: number; text: string }[] = [];
  const position = new Map<string, number>();
  userMsgs.forEach(({ id, text }, at) => {
    const t = text.trim();
    position.set(id, at);
    forms.add(t);
    forms.add(attachmentCaption(t));
    for (const line of t.split('\n')) forms.add(line.trim());
    if (t.includes('\n')) blocks.push({ at, text: `\n${t}\n` });
  });
  return (sent, afterId) => {
    const s = sent.trim();
    const cap = attachmentCaption(sent);
    if (forms.has(s) || (!!cap && forms.has(cap))) return true;
    if (!s.includes('\n')) return false;
    // Mensagens que chegam no meio do turno o Claude grava num `user` só, separadas por quebra de linha.
    const from = afterId === undefined ? -1 : position.get(afterId) ?? -1;
    return blocks.some((b) => b.at > from && b.text.includes(`\n${s}\n`));
  };
}
