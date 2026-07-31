<script lang="ts">
  import { renderMarkdown } from '../lib/markdown';
  import { parseFilePaths, parseMediaUrls, splitTodoBlock } from '../lib/format';
  import { copyText } from '../lib/clipboard';
  import FileAttachment from './FileAttachment.svelte';

  interface Props {
    text: string;
    ts?: number | null;
    sessionName?: string;
    preview?: boolean;
    animate?: boolean;   // false = bubble de HISTORICO remontada (paginacao/janela): sem fade/slide
    onForward?: (() => void) | null; // abre o picker "encaminhar pra sessao" (botao ↗)
  }
  let { text, ts, sessionName = '', preview = false, animate = true, onForward = null }: Props = $props();

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

<!-- ponytail: sem long-press aqui de proposito — o timer de 500ms roubava o gesto de SELECIONAR
     texto do iOS (segurar abria a sheet de encaminhar). As acoes moram na linha do horario. -->
<div class="assistant-msg" class:noanim={!animate}>
  {#if preview}
    <!-- Preview ao vivo: texto PLANO (markdown so no snap final canonico, pra nao piscar **/code-fence
         meio-aberto) + caret. Mesma casca da bolha real -> swap quase invisivel. -->
    {@const todo = splitTodoBlock(text)}
    {#if todo}
      <!-- Painel de tarefas do TUI: fechado por padrao, so o contador na linha. <details> nativo —
           sem estado no componente, e o navegador ja lembra do aberto enquanto o no viver. -->
      <details class="todo-fold">
        <summary>{todo.head}</summary>
        <pre class="todo-body">{todo.body}</pre>
      </details>
    {/if}
    {#if !todo || todo.rest}
      <div class="prose plain">{todo ? todo.rest : text}<span class="caret" aria-hidden="true"></span></div>
    {/if}
  {:else}
    <!-- eslint-disable-next-line svelte/no-at-html-tags -->
    <div class="prose" onclick={onProseClick} role="presentation">{@html html}</div>
    {#if fileRefs.length}<FileAttachment {sessionName} refs={fileRefs} />{/if}
    {#if mediaRefs.length}<FileAttachment {sessionName} refs={mediaRefs} />{/if}
    <div class="msg-actions">
      {#if ts}
        <span class="ts">{formatTime(ts)}</span>
      {/if}
      <button class="msg-copy" class:copied={msgCopied} onclick={copyMessage} aria-label="Copiar mensagem" title="Copiar mensagem"></button>
      {#if onForward}
        <button class="msg-fwd" onclick={onForward} aria-label="Encaminhar pra outra sessão" title="Encaminhar pra outra sessão"></button>
      {/if}
    </div>
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

  /* Ações da mensagem (copiar / encaminhar) na linha do horário: no toque ficam SEMPRE visíveis
     (é a única forma de encaminhar agora que o long-press saiu); no desktop aparecem no hover. */
  .msg-actions {
    display: flex; align-items: center; gap: var(--space-1);
    margin-top: var(--space-1);
  }
  /* No celular estes sao o principal jeito de tirar codigo da conversa: 28px a 50% de opacidade
     era alvo curto e quase invisivel. No mouse eles seguem escondidos ate o hover (regra abaixo). */
  .msg-copy, .msg-fwd {
    width: 34px; height: 34px; padding: 0;
    display: flex; align-items: center; justify-content: center;
    border: none; border-radius: var(--radius-sm);
    background: transparent; color: var(--text-secondary);
    opacity: 0.75; transition: opacity 120ms var(--ease-out), background 120ms var(--ease-out);
    cursor: pointer;
    -webkit-tap-highlight-color: transparent;
  }
  .msg-copy::before { content: '⧉'; font-size: 16px; line-height: 1; }
  .msg-fwd::before { content: '↗'; font-size: 16px; line-height: 1; }
  .msg-copy.copied { color: var(--accent); opacity: 1; }
  .msg-copy.copied::before { content: '✓'; }

  @media (hover: hover) and (pointer: fine) {
    .msg-copy, .msg-fwd { opacity: 0; }
    .assistant-msg:hover .msg-copy, .assistant-msg:hover .msg-fwd { opacity: 0.55; }
    .msg-copy:hover, .msg-fwd:hover { opacity: 1 !important; background: var(--bg-hover); color: var(--text-primary); }
  }

  .prose {
    color: var(--text-primary);
    /* Usa a largura toda da coluna (ate ao teto de .messages-inner). Sem cap de medida (80ch): em
       tela grande o texto/tabela/code precisam ocupar o espaco — cap deixava metade direita vazia. */
    max-width: 100%;
    word-break: break-word;
    /* As tres medidas do texto sao ajustaveis em Aparencia -> Texto da conversa. A escala e
       multiplicada AQUI, sobre o padrao desta tela, em vez de um valor absoluto: o celular e o
       desktop tem numeros diferentes de proposito (ver a media query abaixo), e um valor unico
       apagaria essa diferenca. Sem preferencia salva, o fallback 1 devolve exatamente o de antes. */
    font-size: calc(var(--text-base) * var(--cp-text-scale, 1));
    line-height: calc(1.6 * var(--cp-lh-scale, 1));
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
    /* Quebrando de linha, o fundo/padding/raio sao UMA caixa cortada ao meio: o 1o pedaco fica sem
       padding a direita e o 2o sem padding a esquerda (duas pilulas serradas). `clone` repete a
       decoracao inteira em cada pedaco. */
    box-decoration-break: clone;
    -webkit-box-decoration-break: clone;
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

  /* Com foto de fundo, o bloco de código era a ÚNICA superfície chapada no meio da conversa: todo o
     resto (painéis, composer, vidro) anda com o slider Transparência, e ele ficava sólido, lendo como
     recorte colado por cima da foto. No terminal esse bloco nem existe — é só texto indentado. Aqui a
     caixa vale (delimita o que é comando), então ela fica, mas participando do véu como os painéis.
     `--cp-panel-alpha` já é a alfa que o slider governa (lib/background.ts:47).
     `:not(pre) > code` e não `code` solto: o fence gera <pre><code>, e a regra `pre code` logo abaixo
     zera o fundo do code de dentro de propósito (senão são DUAS caixas empilhadas). Com `code` solto
     esta regra tem especificidade maior, ganhava daquela e o fundo voltava — e como o code é `inline`,
     ele pinta a largura do TEXTO de cada linha, não do bloco: faixas mais opacas linha a linha, só no
     modo com foto. Medido: code a 0,84 sobre pre a 0,84, 689px contra 830px. */
  :global(html[data-bg='image']) .prose :global(pre),
  :global(html[data-bg='image']) .prose :global(:not(pre) > code),
  :global(html[data-bg='image']) .prose :global(.copy-btn) {
    background: color-mix(in srgb, var(--bg-elevated) calc(var(--cp-panel-alpha, 0.87) * 100%), transparent);
  }

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
  .prose :global(li) { line-height: 1.6; margin: 2px 0; }

  .prose :global(a) { color: var(--accent); text-decoration: underline; }

  /* ── Leitura em linha longa ─────────────────────────────────────────────
     SEM cap de medida: largura cheia e decisao registrada no DESIGN.md, e no uso real (texto
     tecnico intercalado com codigo e saida de comando) a coluna estreita custa mais do que ajuda —
     ainda deixava um vao morto a direita da prosa enquanto o codigo ao lado usava tudo.
     O que a linha longa realmente cobra e o RETORNO de linha: o olho volta da direita e erra o
     comeco da linha de baixo. Isso se paga com entrelinha, que nao custa espaco horizontal. 17px
     tambem reduz a contagem por linha sozinho (103 -> ~97 na mesma coluna) e aumenta a letra. */
  /* Desktop: a coluna e larga, entao a linha e longa — e linha longa se paga com ENTRELINHA, que
     nao custa espaco horizontal. 17px ainda reduz a contagem por linha sozinho (103 -> ~97 na mesma
     largura) e aumenta a letra. No celular nada disso vale: a linha ja e curta por falta de espaco
     e 16px e o tamanho certo pra tela pequena. */
  @media (min-width: 820px) {
    .prose {
      font-size: calc(17px * var(--cp-text-scale, 1));
      line-height: calc(1.7 * var(--cp-lh-scale, 1));
    }
  }

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
  /* Sobre papel de parede a tabela NAO vira caixa: quem segura a leitura e a GRADE, nao um fundo.
     Caixa aqui seria o oposto do resto da tela — a conversa inteira e texto direto sobre a foto, e
     um retangulo so pra tabela le como recorte colado. O cabecalho tambem larga o fundo proprio,
     senao ele fica sendo a unica caixa da tabela. */
  :global(html[data-bg='image']) .prose :global(th) {
    background: transparent;
  }
  /* Grade BRANCA, nao a hairline acinzentada de 7%: dentro da caixa, sobre foto, a linha cinza some
     e as colunas colam uma na outra. Branco a 30% desenha a grade sem virar tabela de planilha. */
  :global(html[data-bg='image']) .prose :global(th),
  :global(html[data-bg='image']) .prose :global(td) {
    border-color: rgba(255, 255, 255, 0.30);
  }
  :global(html[data-theme='light'][data-bg='image']) .prose :global(th),
  :global(html[data-theme='light'][data-bg='image']) .prose :global(td) {
    border-color: rgba(40, 32, 28, 0.34);
  }

  .ts {
    font-size: var(--text-xs);
    color: var(--text-muted);
    margin-right: var(--space-1);
  }

  /* Preview plano: preserva quebras de linha do pane (sem markdown -> sem blocos). */
  .prose.plain { white-space: pre-wrap; }

  /* Painel de tarefas do TUI dentro do preview: uma linha fechada, arvore ao abrir. SEM caixa —
     nada no fluxo do chat tem superficie propria (bolha do assistente e texto solto, ToolGroup e
     cabecalho + arvore), entao uma caixa aqui viraria o unico retangulo boiando sobre o papel de
     parede. Mesmo idioma do .tg-head do ToolGroup: xs, muted, clicavel. */
  .todo-fold { align-self: stretch; margin-bottom: var(--space-1); }
  .todo-fold summary {
    cursor: pointer; list-style: none;
    padding: var(--space-1) 0; font-size: var(--text-xs); line-height: 1.5; color: var(--text-muted);
  }
  .todo-fold summary::-webkit-details-marker { display: none; }
  .todo-fold summary::before { content: '▸ '; }
  .todo-fold[open] summary::before { content: '▾ '; }
  .todo-fold summary:hover { color: var(--text-secondary); }
  /* A arvore ja vem desenhada em box-drawing pelo TUI (├─/└─) — aqui e so mono + muted pra ela
     alinhar. Nao e o ::before/::after do ToolGroup: la o galho e CSS porque os filhos sao eventos. */
  .todo-body {
    margin: 0; padding: 0 0 var(--space-1); overflow-x: auto;
    font-family: var(--font-mono); font-size: var(--text-xs); line-height: 1.6;
    color: var(--text-muted);
  }

  /* Caret piscando no fim do preview ao vivo (familia Respiracao "Digitando"). */
  .caret {
    display: inline-block; width: 7px; height: 1.05em; vertical-align: -2px;
    margin-left: 2px; border-radius: 1px; background: var(--accent);
    animation: caret-blink 1s steps(1) infinite;
  }
  @keyframes caret-blink { 50% { opacity: 0; } }
</style>
