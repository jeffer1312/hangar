<script lang="ts">
  import { tick } from 'svelte';
  import type { ChatEvent, StateEvent } from '../lib/types';
  import UserBubble from './UserBubble.svelte';
  import AssistantBubble from './AssistantBubble.svelte';
  import ToolCard from './ToolCard.svelte';
  import OptionButtons from './OptionButtons.svelte';
  import Spinner from './Spinner.svelte';
  import ImageBubble from './ImageBubble.svelte';
  import FileAttachment from './FileAttachment.svelte';
  import { parseImageMessage, parseFilePaths } from '../lib/format';
  import { transcriptImageUrl, uploadUrl } from '../lib/api';
  import { windowStartFor, nextWindowEnd } from '../lib/window';

  interface Props {
    events: ChatEvent[];
    stateEvent: StateEvent | null;
    pending: { id: string; text: string; solid?: boolean }[];
    sessionName: string;
    dockH: number;
    preview?: string;
    onSelectOption: (i: number) => void;
    onCancel: () => void;
  }

  let { events, stateEvent, pending, sessionName, dockH, preview = '', onSelectOption, onCancel }: Props = $props();

  let listEl: HTMLElement | undefined = $state();
  let previewEl: HTMLElement | undefined = $state();
  // O usuario "gruda" no fim por padrao; ao rolar pra cima, paramos de arrastar.
  let atBottom = $state(true);

  // Janela de render: monta SO os ultimos WINDOW eventos (a cauda). Sessao longa/compactada (milhares de
  // linhas no .jsonl) montando tudo = tempestade de mount/layout = congela no celular. windowEnd inicia
  // SINCRONO em events.length (o prop ja vem populado: o Chat so monta o MessageList apos loadHistory) ->
  // ja no PRIMEIRO paint a fatia e a cauda, sem montar os 5000 e so depois encolher.
  // WINDOW = botao de calibragem (ajuste no device real); tool_result e filtrado depois, entao bolhas < WINDOW.
  const WINDOW = 120;
  let windowEnd = $state(events.length);

  // Mantem o box do preview rolado no fundo: a cauda desliza DENTRO da caixa (altura fixa) em vez de
  // empurrar/pular o layout do chat. Roda a cada update do preview.
  $effect(() => {
    void preview;
    if (previewEl) previewEl.scrollTop = previewEl.scrollHeight;
  });

  function onScroll() {
    if (!listEl) return;
    const gap = listEl.scrollHeight - listEl.scrollTop - listEl.clientHeight;
    atBottom = gap < 64; // threshold ~64px do fim
  }

  // Build a map of tool_use_id -> tool_result
  const toolResults = $derived(
    (() => {
      const m = new Map<string, ChatEvent>();
      for (const ev of events) {
        if (ev.kind === 'tool_result' && ev.tool_use_id) {
          m.set(ev.tool_use_id, ev);
        }
      }
      return m;
    })()
  );

  // Renderiza so tool_use (tool_result vira card) e SO dentro da janela [windowEnd-WINDOW, windowEnd].
  // Fatiamos o array CRU por indice ANTES de filtrar -> windowEnd/length sao indices crus; filtrar a
  // fatia mantem o {#each} keyed (ev.id) valido. toolResults (acima) segue sobre o array INTEIRO, entao
  // um tool_use na janela ainda resolve seu result.
  const visibleEvents = $derived(
    events.slice(windowStartFor(windowEnd, WINDOW), windowEnd).filter(ev => ev.kind !== 'tool_result')
  );

  // Claude trabalhando? -> msgs da fila durável (id "queued-") ficam atenuadas (= na fila).
  const working = $derived(stateEvent?.state === 'working');

  // Auto-scroll APENAS quando ja estamos no fim. NAO depende de stateEvent (o tick do cronometro/status
  // atualiza stateEvent toda hora e arrastaria o scroll-up do usuario).
  $effect(() => {
    const len = events.length;
    void pending.length;
    void dockH; // composer cresceu (anexo/multilinha) -> re-scrolla pra ultima msg limpar o glass
    void preview; // preview cresce token a token -> acompanha o fundo enquanto o usuario esta colado
    // Mantem a janela: encolheu (reset/clear) re-ancora na cauda; colado no fim acompanha a cauda
    // (remonta o topo SO com o usuario no fundo = sem pulo); rolado pra cima congela. Termina: ao
    // escrever windowEnd=len o effect re-roda e nextWindowEnd vira no-op.
    const next = nextWindowEnd(atBottom, len, windowEnd);
    if (next !== windowEnd) windowEnd = next;
    if (!atBottom) return;
    tick().then(scrollToBottom);
  });

  let rafScroll = 0;
  function scrollToBottom() {
    // Coalesce as escritas num rAF: o preview muda a cada ~150ms (e ate token a token), e uma
    // escrita scrollTop=scrollHeight por chunk = tempestade de layout/repaint sincrono = trepidacao
    // (e pressao de repaint que vira bloco preto no iOS). Um rAF + pular quando ja esta no alvo corta isso.
    if (!listEl) return;
    cancelAnimationFrame(rafScroll);
    rafScroll = requestAnimationFrame(() => {
      if (!listEl) return;
      const target = listEl.scrollHeight - listEl.clientHeight;
      if (Math.abs(listEl.scrollTop - target) > 2) listEl.scrollTop = target;
    });
  }
</script>

<section
  class="message-list"
  style="--dock-h: {dockH}px"
  bind:this={listEl}
  onscroll={onScroll}
  aria-label="Mensagens"
>
  <div class="messages-inner">
    {#each visibleEvents as ev (ev.id)}
      {#if ev.kind === 'user_msg' && (ev.text || ev.image_count)}
        {@const img = ev.text ? parseImageMessage(ev.text) : null}
        {#if ev.image_count}
          <!-- Imagem(ns) colada(s) no TERMINAL: thumbnail buscado lazy do .jsonl (base64). -->
          <ImageBubble caption={ev.text ?? ''} srcs={Array.from({ length: ev.image_count }, (_, i) => transcriptImageUrl(sessionName, ev.id, i))} />
        {:else if ev.id.startsWith('queued-')}
          <!-- Msg da fila durável: atenuada enquanto o Claude trabalha (= na fila, ainda nao
               processada); acende solida quando ele fica idle (= aceita). Da o sinal de "quando
               foi aceita" que o usuario pediu. -->
          <div class="queued-row" class:dim={working}>
            {#if img}
              <ImageBubble caption={img.caption} srcs={img.filenames.map((f) => uploadUrl(sessionName, f))} />
            {:else}
              <UserBubble text={ev.text} ts={ev.ts} />
            {/if}
          </div>
        {:else if img}
          <ImageBubble caption={img.caption} srcs={img.filenames.map((f) => uploadUrl(sessionName, f))} />
        {:else}
          <UserBubble text={ev.text} ts={ev.ts} />
          {#if ev.text}{@const fr = parseFilePaths(ev.text)}{#if fr.length}<FileAttachment {sessionName} refs={fr} />{/if}{/if}
        {/if}
      {:else if ev.kind === 'assistant_msg' && ev.text}
        <AssistantBubble text={ev.text} ts={ev.ts} {sessionName} />
      {:else if ev.kind === 'tool_use'}
        <ToolCard event={ev} result={toolResults.get(ev.tool_use_id ?? '') ?? null} />
      {/if}
    {/each}

    {#if preview}
      <!-- Preview ao vivo do bloco em voo: texto PLANO (markdown bonito so no final canonico, pra
           nao piscar ** / code-fence meio-aberto). Sob fullscreen e a CAUDA deslizante do pane ->
           box CONTIDO (altura fixa, rola DENTRO de si) pra a cauda deslizar suave sem PULAR o layout. -->
      <div class="preview-bubble" bind:this={previewEl}>{preview}</div>
    {/if}

    {#if stateEvent?.state === 'working'}
      <Spinner label={stateEvent.label} />
    {/if}

    {#each pending as p (p.id)}
      {@const pimg = parseImageMessage(p.text)}
      <div class="pending-bubble" class:solid={p.solid}>
        {#if pimg}
          <ImageBubble caption={pimg.caption} srcs={pimg.filenames.map((f) => uploadUrl(sessionName, f))} />
        {:else}
          <UserBubble text={p.text} ts={undefined} />
        {/if}
      </div>
    {/each}

    {#if stateEvent?.state === 'awaiting_input' && stateEvent.question}
      <OptionButtons
        question={stateEvent.question}
        options={stateEvent.options ?? []}
        onSelect={onSelectOption}
        onCancel={onCancel}
      />
    {/if}
  </div>
</section>

<style>
  .message-list {
    flex: 1;
    overflow-y: scroll;
    /* GUARDA: no mobile NUNCA pode rolar na horizontal (todo o conteudo desloca). Qualquer elemento
       largo (chip de arquivo, token longo) fica clipado aqui; code-block tem seu proprio overflow-x
       interno (continua rolavel dentro da box). */
    overflow-x: hidden;
    overscroll-behavior-y: contain;
    scroll-behavior: auto;
    /* Anti-glitch de repaint do iOS (bloco PRETO no momentum scroll): fundo solido (area
       nao-pintada vira o bg, nao preto). Removido -webkit-overflow-scrolling:touch (legado) E o
       translateZ — o translateZ criava uma CAMADA que renderizava PRETA quando o iOS nao repintava
       a tempo (por isso o preto era puro, ignorando o bg). O guard no fit() (Chat) tira o thrash. */
    background: var(--bg-base);
    /* O dock (composer glass) flutua sobre a lista (overlap). Padding = altura REAL do dock
       (--dock-h, medido via ResizeObserver no Chat) + respiro, pra ultima msg sempre limpar o
       glass mesmo com anexo/multilinha. ResizeObserver nao dispara na animacao do teclado
       (composer mantem altura), entao nao volta o reflow que glitchava a NavBar. */
    padding-bottom: calc(var(--dock-h, 150px) + var(--space-3));
  }

  .messages-inner {
    padding: var(--space-4) var(--space-4) var(--space-2);
    display: flex;
    flex-direction: column;
    max-width: 600px;
    width: 100%;
    min-width: 0;       /* permite os filhos encolherem em vez de empurrar a largura */
    margin: 0 auto;
  }

  /* Desktop: usa mais largura (aditivo; mobile fica nos 600px). */
  @media (min-width: 820px) {
    .messages-inner { max-width: 920px; }
  }

  /* Bubble enfileirado: ainda nao processado pelo Claude — atenuado ate solidificar. Precisa ser
     flex-column align-end pra que UserBubble/ImageBubble (que dependem de flex-end no pai) fiquem
     a DIREITA como msg do usuario — senao o wrapper block cola tudo na esquerda (cara de assistente). */
  .pending-bubble {
    display: flex;
    flex-direction: column;
    /* align-items default (stretch): o UserBubble (.bubble-wrap) ocupa a largura e alinha o balao
       a direita sozinho; a ImageBubble usa align-self:flex-end. flex-end aqui encolheria o
       bubble-wrap pro min-content -> palavra curta ("sim") quebrava letra a letra. */
    opacity: 0.5;
  }
  /* Solidificado: o Claude ja consumiu a fila -> vira bubble normal (sem atenuar). */
  .pending-bubble.solid {
    opacity: 1;
  }

  /* Msg da fila durável (evento sintetico "queued-"): alinha a direita (como user) e atenua
     enquanto na fila. Acende sozinha quando o Claude fica idle (transition). */
  .queued-row {
    display: flex;
    flex-direction: column;
    /* stretch (default): bubble de texto ocupa a largura e alinha sozinho; imagem usa align-self.
       flex-end encolhia o bubble pro min-content -> "sim" quebrava letra a letra. */
    transition: opacity 240ms var(--ease-out);
  }
  .queued-row.dim {
    opacity: 0.5;
  }

  /* Preview ao vivo (cauda em voo): lado do assistente, texto plano levemente atenuado pra ler como
     "ainda escrevendo". Some quando o assistant_msg canonico chega (reconcile no Chat). */
  .preview-bubble {
    align-self: stretch;
    width: 100%;
    max-width: 100%;
    white-space: pre-wrap;
    word-break: break-word;
    font-size: var(--text-sm);
    line-height: 1.55;
    color: var(--text-secondary);
    opacity: 0.9;
    /* Contido: a cauda desliza DENTRO (altura fixa + auto-scroll no fundo) -> NAO pula o layout. */
    max-height: 9.5em;
    overflow-y: auto;
    -webkit-overflow-scrolling: touch;
    /* marcador "ao vivo": hairline accent a esquerda, distingue da msg final committada */
    border-left: 2px solid var(--accent);
    padding: 2px 0 2px var(--space-3);
  }
  .preview-bubble::-webkit-scrollbar { width: 0; height: 0; }
</style>
