<script lang="ts">
  import { renderMarkdown } from '../lib/markdown';
  import { parseFilePaths, parseMediaUrls } from '../lib/format';
  import { copyText } from '../lib/clipboard';
  import FileAttachment from './FileAttachment.svelte';

  interface Props {
    text: string;
    ts?: number | null;
    sessionName?: string;
    preview?: boolean;
    animate?: boolean;   // false = bubble de HISTORICO remontada (paginacao/janela): sem fade/slide
  }
  let { text, ts, sessionName = '', preview = false, animate = true }: Props = $props();

  const html = $derived(preview ? '' : renderMarkdown(text));
  // Anexos por caminho citado na minha msg (img/video/html/pdf que eu "mandar").
  const fileRefs = $derived(!preview && sessionName ? parseFilePaths(text) : []);
  // Midia remota (URL http) -> preview inline; nao depende do backend/sessionName.
  const mediaRefs = $derived(preview ? [] : parseMediaUrls(text));

  function formatTime(ts: number | null | undefined): string {
    if (!ts) return '';
    return new Date(ts * 1000).toLocaleTimeString('pt-BR', {
      hour: '2-digit',
      minute: '2-digit',
    });
  }

  // Copiar bloco de codigo: handler delegado (o botao vem do {@html}, sem handler Svelte proprio).
  function onProseClick(e: MouseEvent) {
    const btn = (e.target as HTMLElement).closest('.copy-btn');
    if (!btn) return;
    const code = btn.parentElement?.querySelector('pre')?.textContent ?? '';
    copyText(code);
    btn.classList.add('copied');
    setTimeout(() => btn.classList.remove('copied'), 1200);
  }

  // Copiar a MENSAGEM inteira (markdown cru). Botao aparece no hover (desktop).
  let msgCopied = $state(false);
  function copyMessage() {
    copyText(text);
    msgCopied = true;
    setTimeout(() => (msgCopied = false), 1200);
  }
</script>

<div class="assistant-msg" class:noanim={!animate}>
  {#if preview}
    <!-- Preview ao vivo: texto PLANO (markdown so no snap final canonico, pra nao piscar **/code-fence
         meio-aberto) + caret. Mesma casca da bolha real -> swap quase invisivel. -->
    <div class="prose plain">{text}<span class="caret" aria-hidden="true"></span></div>
  {:else}
    <!-- eslint-disable-next-line svelte/no-at-html-tags -->
    <div class="prose" onclick={onProseClick} role="presentation">{@html html}</div>
    {#if fileRefs.length}<FileAttachment {sessionName} refs={fileRefs} />{/if}
    {#if mediaRefs.length}<FileAttachment {sessionName} refs={mediaRefs} />{/if}
    {#if ts}
      <span class="ts">{formatTime(ts)}</span>
    {/if}
    <button class="msg-copy" class:copied={msgCopied} onclick={copyMessage} aria-label="Copiar mensagem" title="Copiar mensagem"></button>
  {/if}
</div>

<style>
  /* Mensagem do assistente SEM bubble: texto full-width (estilo Claude iOS), mais legivel. */
  .assistant-msg {
    position: relative;
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    min-width: 0;        /* cadeia flex encolhe -> filhos (chip de arquivo) truncam, nao estouram */
    max-width: 100%;
    /* Mensagem entrando (familia Respiracao): sobe com spring (overshoot leve), so na bolha do assistente. */
    animation: msg-in 420ms var(--spring) both;
    margin-bottom: var(--space-4);
  }

  @keyframes msg-in {
    from { opacity: 0; transform: translateY(14px) scale(0.96); }
    to   { opacity: 1; transform: none; }
  }

  /* Historico remontado (paginacao pra cima / re-ancorar da janela): entra parado. */
  .assistant-msg.noanim { animation: none; }

  /* Copiar-mensagem: so desktop (hover). Aparece no hover da mensagem, canto sup. direito. */
  .msg-copy {
    position: absolute; top: 0; right: 0;
    width: 26px; height: 26px; padding: 0;
    display: none; align-items: center; justify-content: center;
    border: 1px solid var(--border-subtle); border-radius: var(--radius-sm);
    background: var(--bg-elevated); color: var(--text-secondary);
    opacity: 0; transition: opacity 120ms var(--ease-out);
  }
  .msg-copy::before { content: '⧉'; font-size: 14px; line-height: 1; }
  .msg-copy.copied { color: var(--accent); opacity: 1; }
  .msg-copy.copied::before { content: '✓'; }
  @media (hover: hover) and (pointer: fine) {
    .msg-copy { display: flex; }
    .assistant-msg:hover .msg-copy { opacity: 0.6; }
    .msg-copy:hover { opacity: 1 !important; }
  }

  .prose {
    color: var(--text-primary);
    /* Usa a largura toda da coluna (ate ao teto de .messages-inner). Sem cap de medida (80ch): em
       tela grande o texto/tabela/code precisam ocupar o espaco — cap deixava metade direita vazia. */
    max-width: 100%;
    word-break: break-word;
    font-size: var(--text-base);
    line-height: 1.6;
  }

  .prose :global(p) { margin: 0; }
  /* Linhas consecutivas (sem linha em branco) = mesmo bloco: quase coladas. Paragrafo REAL (linha
     em branco no markdown -> class="para") ganha o respiro maior. Antes era o INVERSO (linha solta
     12px, paragrafo 8px via <br>). */
  .prose :global(p + p) { margin-top: var(--space-1); }
  .prose :global(p.para) { margin-top: var(--space-3); }
  .prose :global(strong) { font-weight: 600; color: var(--text-primary); }
  .prose :global(em) { font-style: italic; color: var(--text-secondary); }

  .prose :global(code) {
    font-family: var(--font-mono);
    font-size: 0.875em;
    background: var(--bg-elevated);
    padding: 2px 5px;
    border-radius: 4px;
    color: var(--text-primary);
  }

  .prose :global(pre) {
    background: var(--bg-elevated);
    border: 1px solid var(--border-subtle);
    border-radius: var(--radius-sm);
    padding: var(--space-3);
    overflow-x: auto;
    margin: var(--space-2) 0;
    -webkit-overflow-scrolling: touch;
  }

  /* Bloco de codigo com botao copiar no canto. */
  .prose :global(.code-block) { position: relative; }
  .prose :global(.copy-btn) {
    position: absolute; top: 6px; right: 6px;
    width: 28px; height: 28px; padding: 0;
    display: flex; align-items: center; justify-content: center;
    border: 1px solid var(--border-subtle); border-radius: var(--radius-sm);
    background: var(--bg-elevated); color: var(--text-secondary);
    cursor: pointer; opacity: 0.65; transition: opacity 120ms var(--ease-out);
  }
  .prose :global(.copy-btn:hover), .prose :global(.copy-btn:active) { opacity: 1; }
  .prose :global(.copy-btn)::before {
    content: '⧉'; font-size: 15px; line-height: 1;
  }
  .prose :global(.copy-btn.copied) { color: var(--accent); opacity: 1; }
  .prose :global(.copy-btn.copied)::before { content: '✓'; }

  .prose :global(pre code) {
    font-family: var(--font-mono);
    font-size: var(--text-sm);
    line-height: 1.3;
    background: none;
    padding: 0;
    border-radius: 0;
  }

  .prose :global(h1), .prose :global(h2), .prose :global(h3),
  .prose :global(h4), .prose :global(h5), .prose :global(h6) {
    font-weight: 600; color: var(--text-primary); line-height: 1.3;
    margin: var(--space-3) 0 var(--space-2);
  }
  .prose :global(h1) { font-size: 1.4em; }
  .prose :global(h2) { font-size: 1.25em; }
  .prose :global(h3) { font-size: 1.1em; }
  .prose :global(h4), .prose :global(h5), .prose :global(h6) { font-size: 1em; }

  .prose :global(ul) { list-style: disc; margin: var(--space-2) 0; padding-left: 1.4em; }
  .prose :global(ol) { list-style: decimal; margin: var(--space-2) 0; padding-left: 1.5em; }
  .prose :global(li) { line-height: 1.5; margin: 2px 0; }

  .prose :global(a) { color: var(--accent); text-decoration: underline; }

  .prose :global(blockquote) {
    border-left: 3px solid var(--border-default); padding-left: var(--space-3);
    margin: var(--space-2) 0; color: var(--text-secondary);
  }

  /* Tabela GFM: o WRAPPER rola na horizontal (box propria; a pagina nao mexe). hairlines discretas. */
  .prose :global(.md-table) {
    display: block; overflow-x: auto; -webkit-overflow-scrolling: touch;
    max-width: 100%; margin: var(--space-2) 0;
  }
  .prose :global(.md-table table) {
    /* largura NATURAL (nao espreme) MAS estica ate a coluna da msg quando ha espaco (desktop);
       rola no wrapper se passar da tela (mobile fica igual). */
    border-collapse: collapse; width: max-content; min-width: 100%; max-width: none; font-size: var(--text-sm);
  }
  .prose :global(th), .prose :global(td) {
    border: 1px solid var(--border-subtle); padding: 6px 10px; text-align: left; vertical-align: top;
    /* piso = nao colapsa pra quebra letra-a-letra; teto = nao vira uma mega-coluna (quebra por palavra). */
    min-width: 4.5em; max-width: 32em; overflow-wrap: break-word;
  }
  .prose :global(th) {
    background: var(--bg-elevated); font-weight: 600; color: var(--text-primary); white-space: nowrap;
  }

  .ts {
    font-size: var(--text-xs);
    color: var(--text-muted);
    margin-top: var(--space-1);
  }

  /* Preview plano: preserva quebras de linha do pane (sem markdown -> sem blocos). */
  .prose.plain { white-space: pre-wrap; }

  /* Caret piscando no fim do preview ao vivo (familia Respiracao "Digitando"). */
  .caret {
    display: inline-block; width: 7px; height: 1.05em; vertical-align: -2px;
    margin-left: 2px; border-radius: 1px; background: var(--accent);
    animation: caret-blink 1s steps(1) infinite;
  }
  @keyframes caret-blink { 50% { opacity: 0; } }
</style>
