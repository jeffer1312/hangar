<script lang="ts">
  import { tick } from 'svelte';
  import type { ChatEvent, StateEvent } from '../lib/types';
  import UserBubble from './UserBubble.svelte';
  import AssistantBubble from './AssistantBubble.svelte';
  import ToolCard from './ToolCard.svelte';
  import OptionButtons from './OptionButtons.svelte';
  import Spinner from './Spinner.svelte';
  import ImageBubble from './ImageBubble.svelte';
  import { parseImageMessage } from '../lib/format';
  import { getBaseUrl } from '../lib/auth';

  interface Props {
    events: ChatEvent[];
    stateEvent: StateEvent | null;
    pending: { id: string; text: string; solid?: boolean }[];
    sessionName: string;
    dockH: number;
    onSelectOption: (i: number) => void;
    onCancel: () => void;
    onScrollActivity?: () => void;
  }

  let { events, stateEvent, pending, sessionName, dockH, onSelectOption, onCancel, onScrollActivity }: Props = $props();

  let listEl: HTMLElement | undefined = $state();
  // O usuario "gruda" no fim por padrao; ao rolar pra cima, paramos de arrastar.
  let atBottom = $state(true);

  function onScroll() {
    if (!listEl) return;
    const gap = listEl.scrollHeight - listEl.scrollTop - listEl.clientHeight;
    atBottom = gap < 64; // threshold ~64px do fim
    onScrollActivity?.(); // avisa o Chat -> desliga o backdrop-filter do glass durante o scroll
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

  // Only render tool_use events (not tool_result — they're merged into tool cards)
  const visibleEvents = $derived(events.filter(ev => ev.kind !== 'tool_result'));

  // Claude trabalhando? -> msgs da fila durável (id "queued-") ficam atenuadas (= na fila).
  const working = $derived(stateEvent?.state === 'working');

  // Auto-scroll APENAS quando ja estamos no fim. NAO depende de stateEvent (o tick do
  // cronometro/status atualiza stateEvent toda hora e arrastaria o scroll-up do usuario).
  $effect(() => {
    void events.length;
    void pending.length;
    void dockH; // composer cresceu (anexo/multilinha) -> re-scrolla pra ultima msg limpar o glass
    if (!atBottom) return;
    tick().then(scrollToBottom);
  });

  function scrollToBottom() {
    if (listEl) {
      listEl.scrollTop = listEl.scrollHeight;
    }
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
      {#if ev.kind === 'user_msg' && ev.text}
        {@const img = parseImageMessage(ev.text)}
        {#if ev.id.startsWith('queued-')}
          <!-- Msg da fila durável: atenuada enquanto o Claude trabalha (= na fila, ainda nao
               processada); acende solida quando ele fica idle (= aceita). Da o sinal de "quando
               foi aceita" que o usuario pediu. -->
          <div class="queued-row" class:dim={working}>
            {#if img}
              <ImageBubble caption={img.caption} srcs={img.filenames.map((f) => `${getBaseUrl()}/api/sessions/${encodeURIComponent(sessionName)}/uploads/${encodeURIComponent(f)}`)} />
            {:else}
              <UserBubble text={ev.text} ts={ev.ts} />
            {/if}
          </div>
        {:else if img}
          <ImageBubble caption={img.caption} srcs={img.filenames.map((f) => `${getBaseUrl()}/api/sessions/${encodeURIComponent(sessionName)}/uploads/${encodeURIComponent(f)}`)} />
        {:else}
          <UserBubble text={ev.text} ts={ev.ts} />
        {/if}
      {:else if ev.kind === 'assistant_msg' && ev.text}
        <AssistantBubble text={ev.text} ts={ev.ts} />
      {:else if ev.kind === 'tool_use'}
        <ToolCard event={ev} result={toolResults.get(ev.tool_use_id ?? '') ?? null} />
      {/if}
    {/each}

    {#if stateEvent?.state === 'working'}
      <Spinner label={stateEvent.label} />
    {/if}

    {#each pending as p (p.id)}
      {@const pimg = parseImageMessage(p.text)}
      <div class="pending-bubble" class:solid={p.solid}>
        {#if pimg}
          <ImageBubble caption={pimg.caption} srcs={pimg.filenames.map((f) => `${getBaseUrl()}/api/sessions/${encodeURIComponent(sessionName)}/uploads/${encodeURIComponent(f)}`)} />
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
    overscroll-behavior-y: contain;
    scroll-behavior: auto;
    /* Anti-glitch de repaint do iOS (bloco PRETO sobre as msgs no momentum scroll, agravado pelo
       backdrop-filter do glass por cima): fundo solido (area nao-pintada vira bg, nao preto) +
       camada propria (translateZ) pra estabilizar a pintura. Removido -webkit-overflow-scrolling
       :touch (legado, fonte conhecida do tile preto no iOS novo). */
    background: var(--bg-base);
    transform: translateZ(0);
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
    margin: 0 auto;
  }

  /* Bubble enfileirado: ainda nao processado pelo Claude — atenuado ate solidificar. Precisa ser
     flex-column align-end pra que UserBubble/ImageBubble (que dependem de flex-end no pai) fiquem
     a DIREITA como msg do usuario — senao o wrapper block cola tudo na esquerda (cara de assistente). */
  .pending-bubble {
    display: flex;
    flex-direction: column;
    align-items: flex-end;
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
    align-items: flex-end;
    transition: opacity 240ms var(--ease-out);
  }
  .queued-row.dim {
    opacity: 0.5;
  }
</style>
