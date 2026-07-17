<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import NavBar from '../components/NavBar.svelte';
  import AssistantBubble from '../components/AssistantBubble.svelte';
  import { openEventStreamForServer, getHistoryTailForServer } from '../lib/api';
  import { listServers, serverColor } from '../lib/auth';
  import type { Server } from '../lib/auth';
  import type { ChatEvent, StateEvent } from '../lib/types';
  import { stateLabels, stateColors, latestAssistantEvent, type CompareId } from '../lib/format';

  // Grade de comparação (feature #11): abre UM EventSource leve por sessão selecionada (reusa o
  // mesmo stream do Chat, só que apontado pro SERVIDOR DONO de cada uma — sessões de servidores
  // diferentes convivem no mesmo relance) e mostra so a ÚLTIMA resposta + preview ao vivo de cada
  // uma. Tocar num card abre a conversa completa; fechar volta pra lista.
  interface Props {
    ids: CompareId[];
    onOpenSession: (name: string, serverId: string) => void;
    onBack: () => void;
  }
  let { ids, onOpenSession, onBack }: Props = $props();

  interface Card {
    serverId: string;
    name: string;
    serverLabel: string;
    color: string;
    events: ChatEvent[];
    previewText: string;
    stateEvent: StateEvent | null;
    invalidServer: boolean; // serverId não existe mais localmente -> não dá pra conectar nem abrir
    offline: boolean;       // stream falhou (rede/backend fora) -> ainda clicável (o chat tenta de novo)
  }

  let cards = $state<Card[]>([]);
  // Bookkeeping puro (não precisa ser reativo) — mesmo padrão do idIndex do Chat.svelte: um Map
  // dentro de $state não ajuda em nada aqui (só o array `events` precisa disparar re-render).
  const idIndexes: Map<string, number>[] = [];
  const streams: EventSource[] = [];

  // Browser capa ~6 conexões HTTP/1.1 por host — SSE ao vivo só pras primeiras MAX_LIVE sessões de
  // CADA servidor (sobra folga pro stream de lista da Sidebar + fetches). A 7ª conexão não "falha":
  // pendura em connecting e BLOQUEIA qualquer request seguinte pro host (history, upload). Cards
  // além do teto mostram a última resposta via /history (sem preview ao vivo nem chip de estado).
  const MAX_LIVE_PER_SERVER = 4;

  async function loadTail(server: Server, i: number) {
    try {
      const evs = await getHistoryTailForServer(server, cards[i].name, 20);
      idIndexes[i] = new Map(evs.map((e, k) => [e.id, k]));
      cards[i] = { ...cards[i], events: evs, offline: false };
    } catch {
      if (cards[i].events.length === 0) cards[i] = { ...cards[i], offline: true };
    }
  }

  function connect(server: Server, i: number) {
    const es = openEventStreamForServer(server, cards[i].name);
    streams.push(es);

    es.addEventListener('message', (e: MessageEvent) => {
      try {
        const ev = JSON.parse(e.data) as ChatEvent;
        const idx = idIndexes[i].get(ev.id);
        const events = cards[i].events.slice();
        if (idx !== undefined) events[idx] = ev;
        else { idIndexes[i].set(ev.id, events.length); events.push(ev); }
        // Bolha real commitou -> derruba o preview (senão duplica com o texto recém chegado).
        const previewText = ev.kind === 'assistant_msg' && ev.text ? '' : cards[i].previewText;
        cards[i] = { ...cards[i], events, previewText, offline: false };
      } catch { /* linha inválida do SSE -> ignora, o próximo evento corrige */ }
    });

    es.addEventListener('state', (e: MessageEvent) => {
      try { cards[i] = { ...cards[i], stateEvent: JSON.parse(e.data) as StateEvent, offline: false }; } catch { /* ignora */ }
    });

    es.addEventListener('preview', (e: MessageEvent) => {
      try {
        const t = (JSON.parse(e.data) as { text?: string }).text ?? '';
        cards[i] = { ...cards[i], previewText: t };
      } catch { /* ignora */ }
    });

    es.addEventListener('reset', () => {
      idIndexes[i].clear();
      cards[i] = { ...cards[i], events: [], previewText: '' };
    });

    es.onerror = () => {
      if (cards[i].events.length === 0) cards[i] = { ...cards[i], offline: true };
    };
  }

  onMount(() => {
    const servers = listServers();
    cards = ids.map((id) => {
      const srv = servers.find((s) => s.id === id.serverId);
      return {
        serverId: id.serverId,
        name: id.name,
        serverLabel: srv?.label ?? id.serverId,
        color: serverColor(id.serverId),
        events: [],
        previewText: '',
        stateEvent: null,
        invalidServer: !srv,
        offline: false,
      };
    });
    idIndexes.length = 0;
    ids.forEach(() => idIndexes.push(new Map()));

    const liveCount = new Map<string, number>();
    ids.forEach((id, i) => {
      const srv = servers.find((s) => s.id === id.serverId);
      if (!srv) return;
      const n = liveCount.get(srv.id) ?? 0;
      if (n < MAX_LIVE_PER_SERVER) {
        liveCount.set(srv.id, n + 1);
        connect(srv, i);
      } else {
        loadTail(srv, i);
      }
    });
  });

  onDestroy(() => {
    for (const es of streams) es.close();
  });
</script>

<NavBar
  title="Comparar"
  subtitle={`${ids.length} ${ids.length === 1 ? 'sessão' : 'sessões'}`}
  showBack={true}
  onBack={onBack}
/>

{#if ids.length === 0}
  <div class="compare-empty"><p>Nenhuma sessão selecionada.</p></div>
{:else}
  <div class="compare-grid">
    {#each cards as c (c.serverId + '::' + c.name)}
      {@const last = latestAssistantEvent(c.events)}
      <button
        class="compare-card"
        disabled={c.invalidServer}
        onclick={() => onOpenSession(c.name, c.serverId)}
        aria-label={`Abrir ${c.name}`}
      >
        <div class="cc-head">
          <span class="cc-dot" style="background: {c.color};" aria-hidden="true"></span>
          <span class="cc-name">{c.name}</span>
          <span class="cc-srv" style="color: {c.color};" title={c.serverLabel}>{c.serverLabel}</span>
          {#if c.stateEvent}
            <span class="cc-state" style="color: {stateColors[c.stateEvent.state]};">{stateLabels[c.stateEvent.state]}</span>
          {/if}
        </div>
        <div class="cc-body">
          {#if c.previewText}
            <AssistantBubble text={c.previewText} preview animate={false} />
          {:else if last}
            <!-- Sem sessionName: FileAttachment resolveria pelo servidor ATIVO, que pode não ser o
                 dono desta sessão num relance cross-server -> tapping abre o chat completo pra isso. -->
            <AssistantBubble text={last.text ?? ''} ts={last.ts} animate={false} />
          {:else if c.invalidServer}
            <p class="cc-empty">servidor não encontrado</p>
          {:else if c.offline}
            <p class="cc-empty">sem conexão</p>
          {:else}
            <p class="cc-empty">sem respostas ainda</p>
          {/if}
        </div>
      </button>
    {/each}
  </div>
{/if}

<style>
  .compare-empty {
    display: flex; align-items: center; justify-content: center;
    padding: var(--space-10) var(--space-4); color: var(--text-secondary);
  }
  .compare-grid {
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
    padding: var(--space-3) var(--space-4) var(--space-10);
  }
  .compare-card {
    display: flex;
    flex-direction: column;
    align-items: stretch;
    text-align: left;
    background: var(--bg-surface);
    border: 1px solid var(--border-default);
    border-radius: var(--radius-lg);
    padding: var(--space-3);
    gap: var(--space-2);
    min-width: 0;
  }
  .compare-card:disabled { opacity: 0.6; }
  @media (hover: hover) {
    .compare-card:not(:disabled):hover { background: var(--bg-hover); }
  }
  .cc-head { display: flex; align-items: center; gap: var(--space-2); min-width: 0; }
  .cc-dot { width: 8px; height: 8px; border-radius: 50%; flex-shrink: 0; }
  .cc-name {
    font-weight: 600; font-size: var(--text-sm);
    white-space: nowrap; overflow: hidden; text-overflow: ellipsis;
  }
  .cc-srv { font-size: var(--text-xs); font-weight: 600; flex-shrink: 0; }
  .cc-state { margin-left: auto; font-size: var(--text-xs); font-weight: 600; flex-shrink: 0; }
  .cc-body { max-height: 260px; overflow-y: auto; -webkit-overflow-scrolling: touch; }
  .cc-empty { color: var(--text-muted); font-size: var(--text-sm); }

  /* Desktop: colunas de verdade lado a lado (mobile fica empilhado, um card por linha). */
  @media (min-width: 820px) {
    .compare-grid {
      display: grid;
      grid-template-columns: repeat(auto-fit, minmax(300px, 1fr));
      align-items: start;
    }
  }
</style>
