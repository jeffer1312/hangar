<script lang="ts">
  import { tick } from 'svelte';
  import type { ChatEvent, StateEvent } from '../lib/types';
  import UserBubble from './UserBubble.svelte';
  import AssistantBubble from './AssistantBubble.svelte';
  import ToolCard from './ToolCard.svelte';
  import OptionButtons from './OptionButtons.svelte';

  interface Props {
    events: ChatEvent[];
    stateEvent: StateEvent | null;
    onSelectOption: (i: number) => void;
    onCancel: () => void;
  }

  let { events, stateEvent, onSelectOption, onCancel }: Props = $props();

  let listEl: HTMLElement | undefined = $state();

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

  // Auto-scroll to bottom when events change
  $effect(() => {
    // Reference events to trigger on change
    void events.length;
    void stateEvent;
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
  aria-label="Mensagens"
>
  <div class="messages-inner">
    {#each visibleEvents as ev (ev.id)}
      {#if ev.kind === 'user_msg' && ev.text}
        <UserBubble text={ev.text} ts={ev.ts} />
      {:else if ev.kind === 'assistant_msg' && ev.text}
        <AssistantBubble text={ev.text} ts={ev.ts} />
      {:else if ev.kind === 'tool_use'}
        <ToolCard event={ev} result={toolResults.get(ev.tool_use_id ?? '') ?? null} />
      {/if}
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
    /* Make room for the fixed bottom dock (statusline bar + composer) + safe area */
    padding-bottom: 160px;
  }

  .messages-inner {
    padding: var(--space-4) var(--space-4) var(--space-2);
    display: flex;
    flex-direction: column;
    max-width: 600px;
    width: 100%;
    margin: 0 auto;
  }
</style>
