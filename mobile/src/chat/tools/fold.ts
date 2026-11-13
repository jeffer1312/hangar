import { toolGroupTitulo, toolPhase, type ChatEvent, type HtmlPageRef, type ItemConversa, type Task } from '@hangar/core';
import * as m from '../../paraglide/messages';

// Linha da lista do chat no layout do nativo: mensagem, ou um trecho de trabalho (raciocínios e
// chamadas seguidas) dobrado numa linha só.
export type ConversationRow =
  | { type: 'event'; id: string; ev: ChatEvent }
  /** `source`: de que item do core o trecho veio; ausente quando a Árvore juntou vários. */
  | { type: 'fold'; id: string; parts: ChatEvent[]; source?: 'tool' | 'group' | 'pensamento' }
  | { type: 'tasks'; id: string; tasks: Task[] }
  /** Página publicada pelo html_render: fora de qualquer trecho, ela existe para ser vista. */
  | { type: 'page'; id: string; ev: ChatEvent; page: HtmlPageRef };

// Na Árvore (`merge`), junta o que o agruparConversa separou (pensamento, chamada solta, grupo)
// quando vem em sequência: entre duas mensagens o nativo mostra UM resumo, não um bloco por tipo.
// No Clássico e nos Chips cada item do core vira o seu trecho. A regra de quem entra no pensamento
// continua no core; aqui só muda o desenho. O id do trecho é o do primeiro item, então fica estável
// enquanto o trecho cresce na cauda durante o streaming. `pageOf` diz se a chamada já publicou uma
// página; rodando ainda ou com erro, ela segue como linha comum do trecho.
export function foldConversation(
  items: ItemConversa[],
  merge = true,
  pageOf: (ev: ChatEvent) => HtmlPageRef | null = () => null,
): ConversationRow[] {
  const rows: ConversationRow[] = [];
  let open: { type: 'fold'; id: string; parts: ChatEvent[] } | null = null;
  for (const item of items) {
    if (item.type === 'event') {
      open = null;
      rows.push(item);
      continue;
    }
    if (item.type === 'tool') {
      const page = pageOf(item.ev);
      if (page) {
        open = null;
        rows.push({ type: 'page', id: `p-${item.id}`, ev: item.ev, page });
        continue;
      }
    }
    const parts = item.type === 'tool' ? [item.ev] : item.type === 'group' ? item.tools : item.eventos;
    if (!merge) rows.push({ type: 'fold', id: `f-${item.id}`, parts, source: item.type });
    else if (open) { for (const part of parts) open.parts.push(part); }
    else {
      open = { type: 'fold', id: `f-${item.id}`, parts: parts.slice() };
      rows.push(open);
    }
  }
  return rows;
}

export interface FoldSummary {
  lines: ChatEvent[];
  calls: number;
  thoughts: number;
  counts: number[];
  failures: number;
  live: Set<ChatEvent>;
}

function family(name: string | null | undefined): number {
  switch (name) {
    case 'Read': case 'NotebookRead': return 0;
    case 'Grep': case 'Glob': case 'WebSearch': case 'WebFetch': return 1;
    case 'Edit': case 'MultiEdit': case 'NotebookEdit': return 2;
    case 'Write': return 3;
    case 'Bash': case 'PowerShell': case 'exec': case 'exec_command': return 4;
    default: return 5;
  }
}

export function appendFoldSummary(before: FoldSummary | undefined, parts: ChatEvent[], resultOf: (ev: ChatEvent) => ChatEvent | null): FoldSummary {
  const out: FoldSummary = before ? { ...before, lines: before.lines.slice(), counts: before.counts.slice(), live: new Set(before.live) }
    : { lines: [], calls: 0, thoughts: 0, counts: [0, 0, 0, 0, 0, 0], failures: 0, live: new Set() };
  return accumulateFoldSummary(out, parts, resultOf);
}

/** Acumula apenas em um resumo ainda não publicado, exclusivo do chamador. */
export function accumulateFoldSummary(out: FoldSummary, parts: ChatEvent[], resultOf: (ev: ChatEvent) => ChatEvent | null): FoldSummary {
  for (const ev of parts) {
    if (ev.kind === 'thinking') { out.thoughts++; out.lines.push(ev); }
    else if (ev.kind === 'tool_use' && ev.tool_name !== 'ToolSearch') {
      out.lines.push(ev); out.calls++; out.counts[family(ev.tool_name)]++;
      const phase = toolPhase(resultOf(ev));
      if (phase === 'pending') out.live.add(ev);
      if (phase === 'error') out.failures++;
    }
  }
  return out;
}

export function updateFoldSummary(before: FoldSummary, uses: ChatEvent[], oldResult: ChatEvent | null, result: ChatEvent | null): FoldSummary {
  const oldPhase = toolPhase(oldResult), phase = toolPhase(result);
  if (oldPhase === phase) return before;
  const out = { ...before, live: new Set(before.live) };
  for (const ev of uses) {
    if (ev.tool_name === 'ToolSearch') continue;
    if (phase === 'pending') out.live.add(ev); else out.live.delete(ev);
    out.failures += Number(phase === 'error') - Number(oldPhase === 'error');
  }
  return out;
}

export function summaryTitle(summary: FoldSummary): string {
  const n = summary.counts;
  const parts = [
    n[0] ? (n[0] === 1 ? m.tool_titulo_leu_1() : m.tool_titulo_leu({ n: n[0] })) : '',
    n[1] ? (n[1] === 1 ? m.tool_titulo_buscou_1() : m.tool_titulo_buscou({ n: n[1] })) : '',
    n[2] ? (n[2] === 1 ? m.tool_titulo_editou_1() : m.tool_titulo_editou({ n: n[2] })) : '',
    n[3] ? (n[3] === 1 ? m.tool_titulo_criou_1() : m.tool_titulo_criou({ n: n[3] })) : '',
    n[4] ? (n[4] === 1 ? m.tool_titulo_rodou_1() : m.tool_titulo_rodou({ n: n[4] })) : '',
    n[5] ? (n[5] === 1 ? m.tool_titulo_outra_1() : m.tool_titulo_outras({ n: n[5] })) : '',
  ].filter(Boolean).join(' · ');
  const thought = !summary.thoughts ? '' : summary.thoughts === 1 ? m.native_thinking() : m.native_tree_thoughts({ n: summary.thoughts });
  const counts = thought ? parts : parts.charAt(0).toUpperCase() + parts.slice(1);
  return thought && counts ? `${thought} · ${counts.charAt(0).toLowerCase()}${counts.slice(1)}` : thought || counts;
}

/** Chamadas do trecho, sem o carregador de ferramentas: "select:WebSearch" é encanamento, não ação. */
export function foldCalls(parts: ChatEvent[]): ChatEvent[] {
  return parts.filter((p) => p.kind === 'tool_use' && p.tool_name !== 'ToolSearch');
}

// Título do trecho como no nativo: o raciocínio primeiro, depois a contagem por família
// ("3 raciocínios · rodou 2 comandos"). O `description` do Bash não entra: ele vira título de grupo
// no core, mas aqui o resumo tem que contar o que houve.
export function foldTitle(parts: ChatEvent[]): string {
  const thoughts = parts.filter((p) => p.kind === 'thinking').length;
  const calls = foldCalls(parts);
  const thought = thoughts === 0 ? '' : thoughts === 1 ? m.native_thinking() : m.native_tree_thoughts({ n: thoughts });
  const counts = calls.length ? toolGroupTitulo(calls.map((c) => ({ tool_name: c.tool_name }))) : '';
  if (thought && counts) return `${thought} · ${counts.charAt(0).toLowerCase()}${counts.slice(1)}`;
  return thought || counts;
}

/** "1 falhou" / "N falharam"; vazio sem falha. */
export function failedLabel(n: number): string {
  return n === 0 ? '' : n === 1 ? m.native_tools_failed_1() : m.native_tools_failed({ n });
}

const FILE_TOOLS = new Set(['Read', 'NotebookRead', 'Write', 'Edit', 'MultiEdit', 'NotebookEdit']);

/** Alvo da linha: nas ferramentas de arquivo só o nome do arquivo, que é o que cabe no celular. */
export function callTarget(name: string | null | undefined, summary: string): string {
  return name && FILE_TOOLS.has(name) ? summary.replace(/^.*\//, '') : summary;
}
