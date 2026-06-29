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

  // Quando o preview ao vivo some (bloco commitou), a ULTIMA bolha de assistente que aparece
  // tomou o lugar dele -> entra sem o slide (no-entrance), pra a troca nao "pular". One-shot.
  let swapNext = $state(false);
  let prevPreview = $state('');
  $effect(() => {
    const now = preview;
    if (prevPreview && !now) swapNext = true;   // falling edge: preview -> vazio
    prevPreview = now;
  });

  // Consome o one-shot: assim que ha evento no fim com swapNext setado, limpa no proximo frame.
  $effect(() => {
    if (swapNext && events.length) {
      const id = requestAnimationFrame(() => { swapNext = false; });
      return () => cancelAnimationFrame(id);
    }
  });

  let listEl: HTMLElement | undefined = $state();
  // O usuario "gruda" no fim por padrao; ao rolar pra cima, paramos de arrastar.
  let atBottom = $state(true);
  // Rolou MUITO pra cima (mais de uma tela do fim) -> mostra o botao "ir pro fim".
  let scrolledUp = $state(false);

  // Janela de render: monta SO os ultimos WINDOW eventos (a cauda). Sessao longa/compactada (milhares de
  // linhas no .jsonl) montando tudo = tempestade de mount/layout = congela no celular. windowEnd inicia
  // SINCRONO em events.length (o prop ja vem populado: o Chat so monta o MessageList apos loadHistory) ->
  // ja no PRIMEIRO paint a fatia e a cauda, sem montar os 5000 e so depois encolher.
  // WINDOW = botao de calibragem (ajuste no device real); tool_result e filtrado depois, entao bolhas < WINDOW.
  const WINDOW = 120;
  const PAGE = 100;            // quantos eventos antigos revelar por vez ao rolar pro topo (paginacao)
  let windowEnd = $state(events.length);
  let extra = $state(0);       // eventos revelados ALEM da janela padrao (cresce ao rolar pro topo)

  function onScroll() {
    if (!listEl) return;
    const gap = listEl.scrollHeight - listEl.scrollTop - listEl.clientHeight;
    atBottom = gap < 64; // threshold ~64px do fim
    scrolledUp = gap > listEl.clientHeight; // mais de uma tela do fim = "muito pra cima" -> botao
    // Perto do topo + ainda ha eventos antigos fora da janela -> revela a proxima pagina.
    if (listEl.scrollTop < 200 && hasOlder) revealOlder();
  }

  let revealing = false;
  async function revealOlder() {
    if (revealing || !listEl || !hasOlder) return;
    revealing = true;
    // Preserva a posicao de leitura: o conteudo cresce PRA CIMA (prepend); mede a altura antes, revela,
    // e empurra o scrollTop pelo delta -> a tela nao "pula" pro topo.
    const prevH = listEl.scrollHeight;
    const prevTop = listEl.scrollTop;
    extra += PAGE;
    await tick();
    if (listEl) listEl.scrollTop = prevTop + (listEl.scrollHeight - prevH);
    revealing = false;
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
  // Inicio da janela = windowEnd - (WINDOW + extra): por padrao so a cauda; cada reveal cresce `extra`,
  // revelando uma pagina de eventos MAIS ANTIGOS (paginacao pra cima). Os antigos JA estao em `events`
  // (o /history carrega tudo) -> revelar e so expandir a fatia, sem chamada ao backend.
  const windowStart = $derived(windowStartFor(windowEnd, WINDOW + extra));
  const hasOlder = $derived(windowStart > 0);   // ainda ha eventos fora da janela (acima)?
  const visibleEvents = $derived(
    events.slice(windowStart, windowEnd).filter(ev => ev.kind !== 'tool_result')
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
    // De volta ao fim (live): re-ancora na janela-cauda, descartando o que foi revelado pra cima ->
    // limita o mount count de novo. So reseta quando colado no fim (lendo historico, extra persiste).
    if (atBottom && extra !== 0) extra = 0;
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
    {#each visibleEvents as ev, i (ev.id)}
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
              <UserBubble text={ev.text ?? ''} ts={ev.ts} />
            {/if}
          </div>
        {:else if img}
          <ImageBubble caption={img.caption} srcs={img.filenames.map((f) => uploadUrl(sessionName, f))} />
        {:else}
          <UserBubble text={ev.text ?? ''} ts={ev.ts} />
          {#if ev.text}{@const fr = parseFilePaths(ev.text)}{#if fr.length}<FileAttachment {sessionName} refs={fr} />{/if}{/if}
        {/if}
      {:else if ev.kind === 'assistant_msg' && ev.text}
        <AssistantBubble
          text={ev.text}
          ts={ev.ts}
          {sessionName}
          noEntrance={swapNext && i === visibleEvents.length - 1}
        />
      {:else if ev.kind === 'tool_use'}
        <ToolCard event={ev} result={toolResults.get(ev.tool_use_id ?? '') ?? null} />
      {/if}
    {/each}

    {#if preview}
      <AssistantBubble text={preview} ts={undefined} preview />
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

{#if scrolledUp}
  <!-- Botao "ir pro fim": aparece so quando rolou muito pra cima. Ao tocar, volta pra cauda E zera a
       paginacao revelada (extra=0) -> nao fica montando/segurando paginas antigas que nao precisam. -->
  <button
    class="to-bottom"
    style="bottom: calc({dockH}px + var(--space-3))"
    onclick={() => { extra = 0; atBottom = true; scrollToBottom(); }}
    aria-label="Ir para a última mensagem"
  >
    <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor"
         stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><path d="M6 9l6 6 6-6" /></svg>
  </button>
{/if}

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
    /* Navbar overlay (glass): a 1a msg limpa a navbar; ao rolar, o conteudo passa POR BAIXO dela. */
    padding-top: calc(var(--nav-h, 56px) + var(--space-3));
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
    transform: scale(0.97);
    transition: opacity 360ms var(--spring), transform 360ms var(--spring);
  }
  /* Solidificado: o Claude ja consumiu a fila -> vira bubble normal (sem atenuar). */
  .pending-bubble.solid {
    opacity: 1;
    transform: none;
  }

  /* Msg da fila durável (evento sintetico "queued-"): alinha a direita (como user) e atenua
     enquanto na fila. Acende sozinha quando o Claude fica idle (transition). */
  .queued-row {
    display: flex;
    flex-direction: column;
    /* stretch (default): bubble de texto ocupa a largura e alinha sozinho; imagem usa align-self.
       flex-end encolhia o bubble pro min-content -> "sim" quebrava letra a letra. */
    transition: opacity 240ms var(--ease-out), transform 360ms var(--spring);
  }
  .queued-row.dim {
    opacity: 0.5;
    transform: scale(0.97);   /* na fila: atenua E encolhe um tico; assenta com spring ao ser aceita. */
  }

  /* Botao flutuante "ir pro fim": fixo no canto, acima do dock (bottom = altura do composer + respiro).
     z acima das msgs, abaixo dos sheets. So aparece quando scrolledUp (rolou +1 tela pra cima). */
  .to-bottom {
    position: fixed;
    right: var(--space-4);
    z-index: 6;
    width: 40px;
    height: 40px;
    border-radius: 50%;
    display: flex;
    align-items: center;
    justify-content: center;
    background: var(--bg-elevated);
    border: 1px solid var(--border-default);
    color: var(--text-primary);
    box-shadow: 0 4px 14px rgba(0, 0, 0, 0.35);
    cursor: pointer;
    -webkit-tap-highlight-color: transparent;
    transition: transform 140ms var(--ease-out), background 140ms var(--ease-out);
  }
  .to-bottom:active {
    transform: scale(0.92);
    background: var(--bg-hover);
  }
</style>
