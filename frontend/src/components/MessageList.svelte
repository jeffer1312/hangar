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
    pending: { id: string; text: string }[];
    sessionName: string;
    onSelectOption: (i: number) => void;
    onCancel: () => void;
  }

  let { events, stateEvent, pending, sessionName, onSelectOption, onCancel }: Props = $props();

  let listEl: HTMLElement | undefined = $state();
  // O usuario "gruda" no fim por padrao; ao rolar pra cima, paramos de arrastar.
  let atBottom = $state(true);

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

  // Only render tool_use events (not tool_result — they're merged into tool cards)
  const visibleEvents = $derived(events.filter(ev => ev.kind !== 'tool_result'));

  // Auto-scroll APENAS quando ja estamos no fim. NAO depende de stateEvent (o tick do
  // cronometro/status atualiza stateEvent toda hora e arrastaria o scroll-up do usuario).
  $effect(() => {
    void events.length;
    void pending.length;
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
  bind:this={listEl}
  onscroll={onScroll}
  aria-label="Mensagens"
>
  <div class="messages-inner">
    {#each visibleEvents as ev (ev.id)}
      {#if ev.kind === 'user_msg' && ev.text}
        {@const img = parseImageMessage(ev.text)}
        {#if img}
          <ImageBubble caption={img.caption} src={`${getBaseUrl()}/api/sessions/${encodeURIComponent(sessionName)}/uploads/${encodeURIComponent(img.filename)}`} />
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
      <div class="pending-bubble">
        <UserBubble text={p.text} ts={undefined} />
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
    -webkit-overflow-scrolling: touch;
    overscroll-behavior-y: contain;
    scroll-behavior: auto;
    /* O dock agora e um flex sibling real (nao fixed) -> sem padding gigante. */
    padding-bottom: var(--space-3);
  }

  .messages-inner {
    padding: var(--space-4) var(--space-4) var(--space-2);
    display: flex;
    flex-direction: column;
    max-width: 600px;
    width: 100%;
    margin: 0 auto;
  }

  /* Bubble enfileirado: ainda nao processado pelo Claude — atenuado ate solidificar. */
  .pending-bubble {
    opacity: 0.5;
  }
</style>
