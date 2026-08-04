<script lang="ts">
  import { renderMarkdown } from '../lib/markdown';
  import { parseFilePaths, parseMediaUrls, splitTodoBlock } from '../lib/format';
  import { copyText } from '../lib/clipboard';
  import { textoFalavelComCodigo } from '../lib/speakable';
  import { abrirComTexto } from '../lib/ttsSelection.svelte';
  import FileAttachment from './FileAttachment.svelte';
  import IconSpeaker from './icons/IconSpeaker.svelte';
  import { highlightCodeBlocks } from '../lib/highlight';

  interface Props {
    text: string;
    ts?: number | null;
    sessionName?: string;
    preview?: boolean;
    md?: boolean;        // previa cujo texto e markdown CRU (veio do agente, nao raspado da tela)
    animate?: boolean;   // false = bubble de HISTORICO remontada (paginacao/janela): sem fade/slide
    onForward?: (() => void) | null; // abre o picker "encaminhar pra sessao" (botao ↗)
  }
  let { text, ts, sessionName = '', preview = false, md = false, animate = true, onForward = null }: Props = $props();

  // Previa em texto PLANO era consequencia da FONTE, nao escolha: raspada do pane, ela ja vinha
  // pintada pela TUI e renderizar de novo estragaria. Quando o proprio agente publica o texto
  // (sidecar do Pi, deltas do Codex) o que chega e markdown CRU — e sem renderizar o usuario le
  // `**negrito**` e `##` na tela, contra a regra do app de markdown nunca aparecer cru.
  // O preco conhecido (e aceito): enquanto o bloco cresce, um `**` ou uma cerca de codigo ainda
  // aberta renderiza como o marcador literal ate fechar. Some sozinho no token seguinte.
  const CARET = '<span class="caret" aria-hidden="true"></span>';

  // O caret tem que entrar DENTRO do ultimo bloco. Solto depois de um `</p>`/`</li>` ele vira item
  // proprio do flex e pisca numa linha vazia abaixo do texto — o mesmo defeito que o ramo de texto
  // plano ja resolve com o <span class="live"> (comentario no template). Nao casou nenhum
  // fechamento conhecido (termina em `</pre>`, `</ul>`)? Vai pro fim mesmo: caret desgrudado e feio,
  // caret nenhum e pior — some o sinal de "ainda escrevendo".
  // Sem superficie de XSS: `renderMarkdown` ja escapa tudo e o CARET e constante nossa.
  function comCaret(h: string): string {
    const m = h.match(/<\/(p|li|h[1-6]|blockquote|td|th)>\s*$/);
    return m && m.index !== undefined ? h.slice(0, m.index) + CARET + h.slice(m.index) : h + CARET;
  }

  const previewHtml = $derived(preview && md ? comCaret(renderMarkdown(text)) : '');
  const html = $derived(preview ? '' : renderMarkdown(text));
  // Anexos por caminho citado na minha msg (img/video/html/pdf que eu "mandar").
  const fileRefs = $derived(!preview && sessionName ? parseFilePaths(text) : []);
  // Midia remota (URL http) -> preview inline; nao depende do backend/sessionName.
  const mediaRefs = $derived(preview ? [] : parseMediaUrls(text));

  // Máscara do topo SÓ com transbordo real — mesmo padrão do .bc-body.masked (BoardCard:333). Ligada
  // sempre, ela apagava a 1ª linha de toda prévia curta, onde não há corte nenhum pra disfarçar.
  let plainEl = $state<HTMLElement | null>(null);
  let plainOverflows = $state(false);
  $effect(() => {
    void text; // prévia é full-replace ~7×/s -> remede a cada troca
    plainOverflows = !!plainEl && plainEl.scrollHeight > plainEl.clientHeight + 4;
  });

  function formatTime(ts: number | null | undefined): string {
    if (!ts) return '';
    return new Date(ts * 1000).toLocaleTimeString('pt-BR', {
      hour: '2-digit',
      minute: '2-digit',
    });
  }

  // Copiar/expandir bloco de codigo: NAO tem handler local — os botoes vem do {@html} e quem
  // responde e o listener GLOBAL de code-actions (lib/codeActions.svelte.ts, montado no App).

  // Syntax highlight dos blocos de codigo: o renderMarkdown e sincrono (devolve texto escapado);
  // quem coloriza e esta passa, depois da montagem, com o Shiki sob demanda (lib/highlight.ts).
  // Idempotente — roda a cada versao do html e so trabalha nos blocos novos.
  $effect(() => {
    html;   // dependencia: nova versao do markdown renderizado
    const el = proseEl;
    if (!el) return;
    void highlightCodeBlocks(el);
  });

  // Copiar a MENSAGEM inteira (markdown cru). Botao aparece no hover (desktop).
  let msgCopied = $state(false);
  function copyMessage() {
    copyText(text);
    msgCopied = true;
    setTimeout(() => (msgCopied = false), 1200);
  }

  let proseEl: HTMLDivElement | undefined = $state();

  // Abre o MESMO painel de narracao guiada da selecao (TtsSelectionPill), com a mensagem inteira
  // como alvo — antes o 🔊 lia direto, mas isso obrigava quem queria "explicar o codigo" a
  // selecionar texto no celular pra chegar no painel, e selecionar no celular e ruim. Sincrono ate
  // aqui (sem await): abrir o painel nao toca audio nenhum, o unlock do iOS acontece dentro dele,
  // no toque que de fato manda tocar (ver ouvirClique/confirmarLeitura na pill).
  // O hint do atalho so aparece no desktop: no celular o botao existe mas Ctrl+Shift+Espaco nao
  // (onGlobalKey do Chat retorna cedo fora do desktop), e tooltip que anuncia tecla morta engana.
  const dicaAtalho = typeof window !== 'undefined' && window.matchMedia('(min-width: 820px)').matches
    ? ' (Ctrl+Shift+Espaço: última resposta visível)' : '';

  function ouvirMensagem() {
    if (!proseEl) return;
    const { texto, blocos } = textoFalavelComCodigo(proseEl);
    abrirComTexto(texto, blocos);
  }
</script>

<!-- ponytail: sem long-press aqui de proposito — o timer de 500ms roubava o gesto de SELECIONAR
     texto do iOS (segurar abria a sheet de encaminhar). As acoes moram na linha do horario. -->
<div class="assistant-msg" class:noanim={!animate} class:preview>
  {#if preview}
    <!-- Preview ao vivo: texto PLANO (markdown so no snap final canonico, pra nao piscar **/code-fence
         meio-aberto) + caret. Mesma casca da bolha real -> swap quase invisivel. -->
    {@const todo = md ? null : splitTodoBlock(text)}
    {#if todo}
      <!-- Painel de tarefas do TUI: fechado por padrao, so o contador na linha. <details> nativo —
           sem estado no componente, e o navegador ja lembra do aberto enquanto o no viver. -->
      <details class="todo-fold">
        <!-- O caret vive na linha do resumo quando o painel é TUDO que veio no preview: o bloco de
             prosa abaixo (que normalmente o carrega) não é renderizado nesse caso, e sem ele a
             bolha ficava sem nenhum sinal de "ainda escrevendo" enquanto o turno durasse. -->
        <summary>{todo.head}{#if !todo.rest}<span class="caret" aria-hidden="true"></span>{/if}</summary>
        <pre class="todo-body">{todo.body}</pre>
      </details>
    {/if}
    {#if md}
      <!-- `livemd` = o mesmo corte por cima do .plain (teto de 10lh, o fim do texto sempre visivel),
           SEM o `pre-wrap` — aqui o conteudo ja e HTML com paragrafo proprio, e o pre-wrap dobraria
           toda quebra. Sem teto, previa longa cresce sem limite e empurra a tela de quem esta lendo:
           e o "pulo" que o corte do outro ramo existe pra evitar. -->
      <!-- eslint-disable-next-line svelte/no-at-html-tags -->
      <div class="prose livemd" class:masked={plainOverflows} bind:this={plainEl}>{@html previewHtml}</div>
    {:else if !todo || todo.rest}
      <!-- O <span> em volta do texto+caret NÃO é decorativo: o .prose.plain é flex (pro corte por
           cima, ver o CSS), e num flex container um nó de texto SOLTO vira item anônimo próprio —
           o caret virava um segundo item, numa linha só dele, em vez de piscar colado na última
           palavra. Com um item único, o conteúdo volta a fluir inline lá dentro. -->
      <div class="prose plain" class:masked={plainOverflows} bind:this={plainEl}><span class="live">{todo ? todo.rest : text}<span class="caret" aria-hidden="true"></span></span></div>
    {/if}
  {:else}
    <!-- eslint-disable-next-line svelte/no-at-html-tags -->
    <div class="prose" bind:this={proseEl}>{@html html}</div>
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
      <!-- Nunca na bolha de preview: aquele texto e full-replace a cada ~150ms (MessageList:274),
           e o audio sairia de um bloco de DOM que ja nao existe mais. -->
      <button class="msg-tts" onclick={ouvirMensagem} aria-label="Ouvir mensagem" title={`Ouvir mensagem${dicaAtalho}`}><IconSpeaker size={15} /></button>
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

  /* A prévia costuma entrar logo abaixo de um ToolGroup, que fecha com 4px (ToolGroup.svelte:81).
     Em texto plano do terminal — sem parágrafo, sem markdown — esses 4px leem como zero e a prévia
     fica grudada no card. Só na prévia: entre mensagens normais o margin-bottom já dá o respiro. */
  .assistant-msg.preview { margin-top: var(--space-2); }

  /* Ações da mensagem (copiar / encaminhar) na linha do horário: no toque ficam SEMPRE visíveis
     (é a única forma de encaminhar agora que o long-press saiu); no desktop aparecem no hover. */
  .msg-actions {
    display: flex; align-items: center; gap: var(--space-1);
    margin-top: var(--space-1);
  }
  /* No celular estes sao o principal jeito de tirar codigo da conversa: 28px a 50% de opacidade
     era alvo curto e quase invisivel. No mouse eles seguem escondidos ate o hover (regra abaixo). */
  .msg-copy, .msg-fwd, .msg-tts {
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
  /* msg-tts leva <IconSpeaker> (SVG) no lugar do emoji 🔊 do ::before: o emoji renderiza colorido
     (azul no Linux/Windows, verde no Android) e destoava dos irmaos ⧉/↗, que sao tinta pura. */
  .msg-copy.copied { color: var(--accent); opacity: 1; }
  .msg-copy.copied::before { content: '✓'; }

  @media (hover: hover) and (pointer: fine) {
    .msg-copy, .msg-fwd, .msg-tts { opacity: 0; }
    .assistant-msg:hover .msg-copy, .assistant-msg:hover .msg-fwd, .assistant-msg:hover .msg-tts { opacity: 0.55; }
    .msg-copy:hover, .msg-fwd:hover, .msg-tts:hover { opacity: 1 !important; background: var(--bg-hover); color: var(--text-primary); }
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

  /* Bloco de codigo: caixa + header (linguagem + copiar + expandir) — o estilo base e GLOBAL
     (app.css), vale pra toda tela que renderiza markdown. Aqui so o que e especifico da bolha. */
  /* O reset global `.code-block pre` (app.css) PERDE em especificidade pra `.prose :global(pre)`
     scoped (0,2,1 > 0,1,1): sem este reset local, o pre dentro da caixa mantinha fundo/borda/margem
     proprios — borda dentro de borda, deslocado pela margem. Caixa dupla, medida no chat. */
  .prose :global(.code-block pre) {
    background: none;
    border: none;
    border-radius: 0;
    margin: 0;
  }

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
  :global(html[data-bg='image']) .prose :global(:not(pre) > code) {
    background: color-mix(in srgb, var(--bg-elevated) calc(var(--cp-panel-alpha, 0.87) * 100%), transparent);
  }
  /* ...mas o pre DENTRO da caixa nao pinta de novo (senao sao DUAS camadas de veu empilhadas).
     Mesma regra no app.css pras outras telas; aqui com especificidade maior pra vencer a de cima. */
  :global(html[data-bg='image']) .prose :global(.code-block pre) { background: none; }

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
  /* TETO de altura da prévia ao vivo. Ela é uma cópia do que o terminal desenha AGORA, trocada
     inteira ~7×/s, e o pane alterna entre uma frase e um painel de 13 itens (que no celular quebra
     em ~35 linhas). Como a lista fica colada no fim, cada troca empurrava e puxava dezenas de
     linhas debaixo de quem estava lendo — o "pulo". Com teto, o bloco cresce até 10 linhas e para.
     O excesso sai por CIMA (justify-content: flex-end): o que interessa na prévia é o FIM, o começo
     já foi lido, e o texto inteiro chega na bolha canônica quando o turno commita. Máscara no topo
     pra cortada não parecer texto faltando — mesma ideia do .bc-body.masked do card. */
  .prose.plain {
    white-space: pre-wrap;
    display: flex; flex-direction: column; justify-content: flex-end;
    max-height: 10lh; overflow: hidden;
  }
  .prose.livemd {
    display: flex; flex-direction: column; justify-content: flex-end;
    max-height: 10lh; overflow: hidden;
  }
  /* Só quando há corte de verdade (class:masked). Ver o comentário do plainOverflows no script. */
  .prose.plain.masked,
  .prose.livemd.masked { mask-image: linear-gradient(to bottom, transparent, black 1.6lh); }

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
