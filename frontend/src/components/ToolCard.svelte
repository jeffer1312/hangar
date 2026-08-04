<script lang="ts">
  import type { ChatEvent } from '../lib/types';
  import { parseFilePaths, summarizeToolInput, summarizeToolResult, toolPhase } from '../lib/format';
  import { extractEdits, extractEditPath } from '../lib/editdiff';
  import FileAttachment from './FileAttachment.svelte';
  import EditDiff from './EditDiff.svelte';

  interface Props {
    event: ChatEvent;
    result?: ChatEvent | null;
    sessionName: string;
    animate?: boolean;   // false = card de HISTORICO remontado (paginacao/janela): sem fade
  }
  let { event, result = null, sessionName, animate = true }: Props = $props();

  // Edit/MultiEdit: o tool_input ja traz old_string/new_string -> da pra mostrar o DIFF (estilo
  // Pi, lado a lado) no lugar do resultado cru. Aberto por padrao, recolhe no toque (escolha do
  // usuario 2026-08-04). null = shape desconhecido -> comportamento de sempre (pre com o result).
  const editEdits = $derived(extractEdits(event.tool_name, event.tool_input));
  const editPath = $derived(extractEditPath(event.tool_input));
  // svelte-ignore state_referenced_locally -- o valor inicial E a decisao (aberto por padrao pra
  // Edit/MultiEdit); o componente e recriado por evento (key do each no MessageList), nao reage.
  let expanded = $state(!!extractEdits(event.tool_name, event.tool_input));

  // Imagem (ou midia/doc) que o Claude LEU: o transcript dropa o bloco image do tool_result, mas o
  // path do Read esta citado na conversa -> serve pelo /file (parseFilePaths filtra por extensao
  // conhecida, entao codigo/.md nao vira anexo). Reusa a mesma trava + componente do chat.
  const fileRefs = $derived(
    event.tool_name === 'Read'
      ? parseFilePaths(String((event.tool_input as Record<string, unknown> | null)?.['file_path'] ?? ''))
      : []
  );

  const phase = $derived(toolPhase(result));
  // Erro mostra o TEXTO do erro (o diff esconderia a mensagem que importa).
  const showDiff = $derived(!!editEdits && phase !== 'error');

  const summary = $derived(summarizeToolInput(event.tool_name, event.tool_input));

  // Desfecho na 2a linha ("Pronto (38 linhas)" / "320 linhas carregadas" / 1a linha do erro).
  // Enquanto roda nao ha resultado -> a linha mostra o proprio estado.
  const outcome = $derived(summarizeToolResult(result, event.tool_name) || 'Executando…');

  // Bash com run_in_background retorna NA HORA (vira shell destacado) -> "Pronto" engana. Marca
  // como background; a saida viva chega depois pelos cards de BashOutput (o agente puxa via bash_id).
  const isBackground = $derived(
    event.tool_name === 'Bash' && (event.tool_input as Record<string, unknown> | null)?.['run_in_background'] === true
  );
</script>

<!-- Bloco de DUAS linhas (layout do Pi): "● Bash <arg>" / "└ Pronto (38 linhas) • toque para ver".
     A linha 2 nao usa o caractere └: o corner e desenhado em CSS (border), que alinha na bolinha em
     qualquer fonte/tamanho e nunca cai num glifo de fallback torto. -->
<div
  class="tool-row"
  class:noanim={!animate}
  class:tool-row--error={phase === 'error'}
  role="button"
  tabindex="0"
  aria-expanded={expanded}
  onclick={() => (expanded = !expanded)}
  onkeydown={(e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); expanded = !expanded; } }}
>
  <div class="tr-call">
    <span class="tr-dot" class:pending={phase === 'pending'} data-phase={phase} aria-hidden="true"></span>
    <span class="tr-name">{event.tool_name ?? 'Tool'}</span>
    {#if isBackground}<span class="tr-badge">background</span>{/if}
    {#if summary}<span class="tr-arg" class:open={expanded}>{summary}</span>{/if}
  </div>

  <div class="tr-out">
    <span class="tr-elbow" aria-hidden="true"></span>
    <span class="tr-outcome">{outcome}</span>
    {#if result?.result || showDiff}
      <span class="tr-hint">
        <span class="sep" aria-hidden="true">•</span>
        <span class="coarse">{expanded ? 'toque para ocultar' : 'toque para ver'}</span><span
              class="fine">{expanded ? 'clique para ocultar' : 'clique para ver'}</span>
      </span>
    {/if}
  </div>

  {#if expanded && showDiff && editEdits}
    <div class="row-result row-result--diff">
      <EditDiff path={editPath} edits={editEdits} />
    </div>
  {:else if expanded && result?.result}
    <div class="row-result">
      <pre>{result.result}</pre>
    </div>
  {/if}
</div>

{#if fileRefs.length}<FileAttachment {sessionName} refs={fileRefs} />{/if}

<style>
  .tool-row {
    padding: var(--space-1) 0;
    margin-bottom: var(--space-1);
    cursor: pointer;
    animation: bubble-in 180ms ease-out both;
  }

  /* Historico remontado (paginacao/janela): entra parado. */
  .tool-row.noanim { animation: none; }

  /* Linha 1: bolinha + nome + argumento. */
  .tr-call {
    display: flex;
    align-items: baseline;
    gap: 6px;
    min-width: 0;
    font-size: var(--text-xs);
    line-height: 1.5;
  }

  /* A bolinha carrega o mesmo vocabulario de estado do resto do app: accent = rodando,
     success = concluiu, error = falhou. */
  .tr-dot {
    flex-shrink: 0;
    width: 6px;
    height: 6px;
    border-radius: 50%;
    align-self: center;
    background: var(--success);
  }
  .tr-dot[data-phase='pending'] { background: var(--accent); }
  .tr-dot[data-phase='error']   { background: var(--error); }
  /* Pulso no lugar do antigo spinner ⟳ (mesma informacao, sem roubar largura da linha). */
  .tr-dot.pending { animation: pulse-scale 1.2s ease-in-out infinite; }

  .tr-name {
    flex-shrink: 0;
    font-weight: 600;
    color: var(--text-secondary);
  }

  .tr-arg {
    min-width: 0;
    font-family: var(--font-mono);
    color: var(--text-muted);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  /* Expandido, o argumento cortado no "…" aparece inteiro (o comando longo e metade do porque). */
  .tr-arg.open { white-space: pre-wrap; overflow: visible; word-break: break-all; }

  .tr-badge {
    flex-shrink: 0;
    font-size: 9px;
    font-weight: 600;
    letter-spacing: 0.03em;
    text-transform: uppercase;
    padding: 1px 6px;
    border-radius: var(--radius-full);
    color: var(--accent);
    background: var(--accent-dim);
  }

  /* Linha 2: corner + desfecho + dica. */
  .tr-out {
    display: flex;
    align-items: baseline;
    gap: 6px;
    min-width: 0;
    font-size: var(--text-xs);
    line-height: 1.5;
    color: var(--text-muted);
  }

  /* O "└" desenhado: sobe ate a bolinha da linha de cima e vira pra direita. */
  .tr-elbow {
    flex-shrink: 0;
    width: 6px;
    height: 8px;
    margin-right: 2px;
    align-self: flex-start;
    border-left: 1px solid var(--border-default);
    border-bottom: 1px solid var(--border-default);
    border-bottom-left-radius: 3px;
  }

  .tr-outcome {
    min-width: 0;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  /* Dica de expandir: some primeiro quando a linha aperta (o desfecho vale mais). */
  .tr-hint {
    flex-shrink: 1000;
    min-width: 0;
    white-space: nowrap;
    overflow: hidden;
    color: var(--text-muted);
    opacity: 0.7;
  }
  .tr-hint .sep { margin-right: 4px; }
  /* "toque" com dedo, "clique" com mouse — o app nao tem atalho de teclado pra isto, entao a dica
     nao inventa um. */
  .fine { display: inline; }
  .coarse { display: none; }
  @media (pointer: coarse) {
    .fine { display: none; }
    .coarse { display: inline; }
  }

  .tool-row--error .tr-outcome { color: var(--error); }

  .row-result {
    margin-top: var(--space-2);
    margin-left: 14px;
    max-height: 240px;
    overflow-y: auto;
    -webkit-overflow-scrolling: touch;
    border-left: 2px solid var(--border-default);
    padding-left: var(--space-3);
  }

  .row-result pre {
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    color: var(--text-secondary);
    line-height: 1.35;
    white-space: pre-wrap;
    word-break: break-all;
  }

  /* O diff ja tem a propria moldura/superficie — sem a bordinha lateral do resultado cru, e sem
     o teto de 240px (o EditDiff tem o proprio scroll interno). */
  .row-result--diff {
    border-left: none;
    padding-left: 0;
    max-height: none;
    overflow-y: visible;
  }
</style>
