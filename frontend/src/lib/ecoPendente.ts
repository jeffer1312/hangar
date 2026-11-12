import { attachmentCaption } from '@hangar/core';

/** O eco local sai quando o transcript traz a mensagem: inteira, pela legenda (sem o "📎 imagem: <path>") ou por uma linha. */
export function ecoEntregue(userTexts: string[]): (sent: string) => boolean {
  const forms = new Set<string>();
  const blocks: string[] = [];
  for (const raw of userTexts) {
    const t = raw.trim();
    forms.add(t);
    forms.add(attachmentCaption(t));
    for (const line of t.split('\n')) forms.add(line.trim());
    if (t.includes('\n')) blocks.push(`\n${t}\n`);
  }
  return (sent) => {
    const s = sent.trim();
    const cap = attachmentCaption(sent);
    // Mensagens que chegam no meio do turno o Claude grava num `user` só, separadas por quebra de linha.
    return forms.has(s) || (!!cap && forms.has(cap)) || (s.includes('\n') && blocks.some((b) => b.includes(`\n${s}\n`)));
  };
}
