<script lang="ts">
  import OptionButtons from './OptionButtons.svelte';
  import { selectOption } from '../lib/api';
  import { getActiveId, selectServer } from '../lib/auth';
  import { attentionFeed } from '../lib/format';
  import type { AggSession } from '../lib/types';

  // "Precisa de você" (feature #6): seção FIXA no topo da lista (mobile + desktop), com TODA sessão
  // AGUARDANDO de TODOS os servidores, mesclada e ordenada por quem espera há mais tempo. Responder
  // aqui NÃO abre o chat: picker de TUI (tem `options`) -> OptionButtons inline, roteado pro servidor
  // DONO (mesmo padrão save/selectServer/restore do delete cross-server) reusando selectOption
  // (endpoint /select); AskUserQuestion nativo (sem `options`) -> abre o chat, onde o stepper já
  // aparece (o SSE reemite ask_question ao conectar). Ao responder, o SSE de sessions tira a sessão
  // de awaiting e a seção encolhe sozinha.
  interface Props {
    sessions: AggSession[];
    onOpenChat: (s: AggSession) => void;
  }
  let { sessions, onOpenChat }: Props = $props();

  const feed = $derived(attentionFeed(sessions));
  const keyOf = (s: AggSession) => `${s.serverId}::${s.name}`;

  let expandedKey = $state<string | null>(null); // entry com OptionButtons aberto (só um por vez)

  function toggle(s: AggSession) {
    if (s.options?.length) {
      const k = keyOf(s);
      expandedKey = expandedKey === k ? null : k; // expande/colapsa inline (sem montar chat)
    } else {
      onOpenChat(s); // AskUserQuestion nativo / sem picker parseável -> stepper no chat
    }
  }

  async function pick(s: AggSession, option: number) {
    expandedKey = null;
    const prev = getActiveId(); // salva antes de mirar o server dono (api.ts lê o ativo a cada chamada)
    selectServer(s.serverId);
    try {
      await selectOption(s.name, option);
    } catch {
      /* SSE de sessions corrige a lista */
    } finally {
      if (prev && prev !== s.serverId) selectServer(prev); // restaura pra o chat aberto ficar no server dele
    }
  }
</script>

{#if feed.length > 0}
  <section class="attn" aria-label="Precisa de você">
    <div class="attn-head">
      <span class="attn-dot" aria-hidden="true"></span>
      <span class="attn-title">Precisa de você</span>
      <span class="attn-count">{feed.length}</span>
    </div>
    {#each feed as s (keyOf(s))}
      {@const open = expandedKey === keyOf(s)}
      <div class="attn-item" class:open>
        <button class="attn-main" onclick={() => toggle(s)}>
          <span class="attn-info">
            <span class="attn-name-row">
              <span class="attn-name">{s.name}</span>
              <span class="attn-srv" style="color: {s.serverColor};" title={s.serverLabel}>{s.serverLabel}</span>
            </span>
            {#if s.question}<span class="attn-q" title={s.question}>{s.question}</span>{/if}
          </span>
          {#if s.options?.length}
            <span class="attn-caret" class:open aria-hidden="true">▾</span>
          {:else}
            <span class="attn-caret" aria-hidden="true">›</span>
          {/if}
        </button>
        {#if open && s.options?.length}
          <OptionButtons
            question={s.question ?? ''}
            options={s.options}
            onSelect={(i) => pick(s, i)}
            onCancel={() => (expandedKey = null)}
          />
        {/if}
      </div>
    {/each}
  </section>
{/if}

<style>
  .attn {
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
    margin: 0 var(--space-4) var(--space-3);
    padding: var(--space-2);
    background: rgba(255, 159, 10, 0.06);
    border: 1px solid rgba(255, 159, 10, 0.22);
    border-radius: var(--radius-lg);
  }
  .attn-head {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    padding: var(--space-1) var(--space-2) var(--space-1);
  }
  .attn-dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--warning);
    flex-shrink: 0;
    animation: attn-pulse 1.8s var(--ease-out) infinite;
  }
  @keyframes attn-pulse {
    0%, 100% { opacity: 1; }
    50% { opacity: 0.35; }
  }
  .attn-title {
    flex: 1;
    font-size: var(--text-xs);
    font-weight: 700;
    color: var(--warning);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  .attn-count {
    font-size: var(--text-xs);
    font-weight: 600;
    color: var(--warning);
    background: rgba(255, 159, 10, 0.14);
    border-radius: var(--radius-full);
    min-width: 20px;
    text-align: center;
    padding: 1px 8px;
  }
  .attn-item {
    display: flex;
    flex-direction: column;
    border-radius: var(--radius-md);
  }
  .attn-item.open {
    background: var(--bg-surface);
  }
  .attn-main {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    min-height: 48px;
    padding: var(--space-2) var(--space-2);
    text-align: left;
    justify-content: flex-start;
    border-radius: var(--radius-md);
  }
  @media (hover: hover) {
    .attn-main:hover { background: var(--bg-hover); }
  }
  .attn-main:active { background: var(--bg-hover); }
  .attn-info {
    display: flex;
    flex-direction: column;
    gap: 1px;
    flex: 1;
    min-width: 0;
  }
  .attn-name-row {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    min-width: 0;
  }
  .attn-name {
    font-size: var(--text-sm);
    font-weight: 600;
    color: var(--text-primary);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .attn-srv {
    flex-shrink: 0;
    font-size: 10px;
    font-weight: 600;
    max-width: 40%;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .attn-q {
    font-size: var(--text-xs);
    color: var(--warning);
    font-weight: 500;
    min-width: 0;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .attn-caret {
    flex-shrink: 0;
    color: var(--text-muted);
    font-size: var(--text-sm);
    transition: transform 160ms var(--ease-out);
  }
  .attn-caret.open { transform: rotate(180deg); }
</style>
