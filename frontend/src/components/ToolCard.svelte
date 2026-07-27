<script lang="ts">
  import type { ChatEvent } from '../lib/types';
  import { parseFilePaths, summarizeToolInput, summarizeToolResult } from '../lib/format';
  import FileAttachment from './FileAttachment.svelte';

  interface Props {
    event: ChatEvent;
    result?: ChatEvent | null;
    sessionName: string;
    animate?: boolean;   // false = card de HISTORICO remontado (paginacao/janela): sem fade
  }
  let { event, result = null, sessionName, animate = true }: Props = $props();

  let expanded = $state(false);

  // Imagem (ou midia/doc) que o Claude LEU: o transcript dropa o bloco image do tool_result, mas o
  // path do Read esta citado na conversa -> serve pelo /file (parseFilePaths filtra por extensao
  // conhecida, entao codigo/.md nao vira anexo). Reusa a mesma trava + componente do chat.
  const fileRefs = $derived(
    event.tool_name === 'Read'
      ? parseFilePaths(String((event.tool_input as Record<string, unknown> | null)?.['file_path'] ?? ''))
      : []
  );

  const phase = $derived<'pending' | 'done' | 'error'>(
    result === null
      ? 'pending'
      : result.is_error
      ? 'error'
      : 'done'
  );

  const verb = $derived(
    phase === 'pending' ? 'Executando' : phase === 'error' ? 'Falhou' : 'Executou'
  );

  const summary = $derived(summarizeToolInput(event.tool_name, event.tool_input));

  // Tamanho do que voltou ("40 linhas") na propria linha colapsada — sem isso so da pra saber
  // expandindo. Erro mostra a 1a linha do erro no lugar da contagem.
  const resultInfo = $derived(summarizeToolResult(result));

  // Bash com run_in_background retorna NA HORA (vira shell destacado) -> "Executou" engana. Marca
  // como background; a saida viva chega depois pelos cards de BashOutput (o agente puxa via bash_id).
  const isBackground = $derived(
    event.tool_name === 'Bash' && (event.tool_input as Record<string, unknown> | null)?.['run_in_background'] === true
  );
  const label = $derived(isBackground && phase !== 'error' ? 'Rodando em background' : verb);
</script>

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
  <div class="row-line">
    {#if phase === 'pending'}
      <span class="row-spin" aria-label="Executando…">⟳</span>
    {/if}
    <!-- Separador como EXPRESSAO ({' · '}): espaco literal na 1a posicao de um {#if} e comido pelo
         compilador, e a linha saia "Bash· cmd: …". -->
    <span class="row-label">
      {label} <span class="row-tool">{event.tool_name ?? 'Tool'}</span>{#if isBackground}<span class="row-badge">background</span>{/if}{#if summary}{' · '}{summary}{/if}
    </span>
    <!-- Fora do .row-label de proposito: no celular a linha e estreita e o argumento come a largura
         toda — dentro do label, o "40 linhas" morria no ellipsis justo onde ele mais serve. -->
    {#if resultInfo}<span class="row-count">{resultInfo}</span>{/if}
    <span class="row-chevron" class:open={expanded} aria-hidden="true">›</span>
  </div>

  {#if expanded && result?.result}
    <div class="row-result">
      <pre>{result.result}</pre>
    </div>
  {/if}
</div>

{#if fileRefs.length}<FileAttachment {sessionName} refs={fileRefs} />{/if}

<style>
  /* Linha muda colapsada (estilo Claude iOS): "Executou <tool> · <summary> ›". Tap expande. */
  .tool-row {
    padding: var(--space-1) 0;
    margin-bottom: var(--space-1);
    cursor: pointer;
    min-height: 32px;
    animation: bubble-in 180ms ease-out both;
  }

  /* Historico remontado (paginacao/janela): entra parado. */
  .tool-row.noanim { animation: none; }

  .row-line {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    min-width: 0;
  }

  .row-spin {
    flex-shrink: 0;
    color: var(--text-muted);
    display: inline-block;
    animation: spin 0.8s linear infinite;
    font-size: var(--text-xs);
  }

  .row-label {
    flex: 1;
    min-width: 0;
    font-size: var(--text-xs);
    color: var(--text-muted);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .row-tool {
    font-family: var(--font-mono);
    color: var(--text-secondary);
  }

  .row-badge {
    flex-shrink: 0;
    font-size: 9px;
    font-weight: 600;
    letter-spacing: 0.03em;
    text-transform: uppercase;
    padding: 1px 6px;
    margin-left: 4px;
    border-radius: var(--radius-full);
    color: var(--accent);
    background: var(--accent-dim);
  }

  /* Tamanho do resultado ("40 linhas") / 1a linha do erro: nunca encolhe abaixo do proprio texto,
     mas nao passa de metade da linha (erro comprido nao pode zerar o argumento). */
  .row-count {
    flex-shrink: 0;
    max-width: 50%;
    font-size: var(--text-xs);
    color: var(--text-muted);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .tool-row--error .row-label,
  .tool-row--error .row-count {
    color: var(--error);
  }

  .row-chevron {
    flex-shrink: 0;
    color: var(--text-muted);
    font-size: var(--text-base);
    transition: transform 180ms var(--ease-out);
  }

  .row-chevron.open {
    transform: rotate(90deg);
  }

  .row-result {
    margin-top: var(--space-2);
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
</style>
