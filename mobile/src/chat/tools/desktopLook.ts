import { nomeFerramenta, toolPhase, toolVerbo, type ChatEvent } from '@hangar/core';
import * as m from '../../paraglide/messages';

export type DesktopFamily = 'web' | 'ferramentas' | 'rodou' | 'leu' | 'editou' | 'criou' | 'buscou' | 'outra';

// A família vem do `toolVerbo` do core: comparar com o verbo de uma ferramenta conhecida segue o
// core sem copiar a tabela dele.
const SAMPLE = { rodou: 'Bash', leu: 'Read', editou: 'Edit', criou: 'Write', buscou: 'Grep' } as const;

export function desktopFamily(name: string | null | undefined): DesktopFamily {
  if (name === 'WebSearch' || name === 'WebFetch') return 'web';
  if (name === 'ToolSearch') return 'ferramentas';
  const verb = toolVerbo(name);
  return (Object.keys(SAMPLE) as Array<keyof typeof SAMPLE>).find((family) => toolVerbo(SAMPLE[family]) === verb) ?? 'outra';
}

function phrase(family: DesktopFamily, n: number, nome: string): string {
  switch (family) {
    case 'rodou': return n === 1 ? m.ferr_desktop_rodou_1() : m.ferr_desktop_rodou({ n });
    case 'leu': return n === 1 ? m.ferr_desktop_leu_1() : m.ferr_desktop_leu({ n });
    case 'editou': return n === 1 ? m.ferr_desktop_editou_1() : m.ferr_desktop_editou({ n });
    case 'criou': return n === 1 ? m.ferr_desktop_criou_1() : m.ferr_desktop_criou({ n });
    case 'buscou': return n === 1 ? m.ferr_desktop_buscou_1() : m.ferr_desktop_buscou({ n });
    case 'web': return n === 1 ? m.ferr_desktop_web_1() : m.ferr_desktop_web({ n });
    case 'ferramentas': return m.ferr_desktop_ferramentas();
    case 'outra': return n === 1 ? m.ferr_desktop_outra_1({ nome }) : m.ferr_desktop_outra({ nome, n });
  }
}

/** "Executou 4 comandos (2 falhas), Leu 1 arquivo, 1 em execução": famílias na ordem em que aparecem. */
export function desktopSummary(calls: ChatEvent[], resultOf: (call: ChatEvent) => ChatEvent | null): string {
  const groups = new Map<string, { family: DesktopFamily; nome: string; n: number; failed: number }>();
  let running = 0;
  for (const call of calls) {
    const family = desktopFamily(call.tool_name);
    const nome = family === 'outra' ? nomeFerramenta(call.tool_name) : '';
    const key = `${family}:${nome}`;
    const group = groups.get(key) ?? { family, nome, n: 0, failed: 0 };
    groups.set(key, group);
    group.n++;
    const phase = toolPhase(resultOf(call));
    if (phase === 'error') group.failed++;
    else if (phase === 'pending') running++;
  }
  const parts = [...groups.values()].map(({ family, nome, n, failed }) => {
    const text = phrase(family, n, nome);
    return failed ? `${text} ${failed === 1 ? m.ferr_desktop_falhas_1() : m.ferr_desktop_falhas({ n: failed })}` : text;
  });
  if (running) parts.push(m.ferr_desktop_executando({ n: running }));
  return parts.join(', ');
}

/** `Exit code N` na primeira linha não vazia do resultado; qualquer outra forma, nada. */
export function exitCode(text: string | null | undefined): number | null {
  const first = (text ?? '').split('\n').find((line) => line.trim())?.trim() ?? '';
  const match = /^Exit code (-?\d+)$/.exec(first);
  return match ? Number(match[1]) : null;
}
