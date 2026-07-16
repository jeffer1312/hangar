<script lang="ts" module>
  import type { SessionInfo } from '../lib/types';
  // Linha do quadro: sessão + servidor dono (pro card falar com o backend certo).
  export interface BoardRow extends SessionInfo { serverId: string }
</script>

<script lang="ts">
  import { onMount } from 'svelte';
  import BoardCard from '../components/BoardCard.svelte';
  import { openSessionsStream } from '../lib/api';
  import { listServers, onServersChanged, serverColor } from '../lib/auth';
  import type { Server } from '../lib/auth';
  import type { State } from '../lib/types';
  import { stateColors } from '../lib/format';

  interface Props { onOpenSession: (name: string, serverId: string) => void }
  let { onOpenSession }: Props = $props();

  // Mesma agregação multi-servidor da Sidebar (slots/recompute/connect): 1 SSE por servidor —
  // NUNCA por card (limite ~6 conexões SSE/host no HTTP/1.1). Falha de um servidor é isolada.
  let servers = $state<Server[]>([]);
  let rows = $state<BoardRow[]>([]);
  let offline = $state<string[]>([]); // labels de servidores sem stream agora
  const slots = new Map<string, { sessions: SessionInfo[] | null; error: string | null }>();
  const streams = new Map<string, EventSource>();

  function recompute() {
    const seen = new Set<string>();
    const out: BoardRow[] = [];
    const off: string[] = [];
    for (const srv of servers) {
      const slot = slots.get(srv.id);
      if (slot?.error) off.push(srv.label);
      if (!slot?.sessions) continue;
      for (const s of slot.sessions) {
        const key = `${s.jsonl ?? s.cwd ?? ''}::${s.name}`; // dedup: backend atrás de 2 URLs
        if (seen.has(key)) continue;
        seen.add(key);
        out.push({ ...s, serverId: srv.id });
      }
    }
    rows = out;
    offline = off;
  }

  // Reconcilia os streams com a lista: fecha o que sumiu, abre o que entrou, mantem o resto (mesmo
  // connect da Sidebar). Reabrir tudo a cada mudanca custaria o historico de sessoes dos servers que
  // ficaram — e a cota de ~6 SSE/host.
  function connect(list: Server[]) {
    for (const [id, es] of streams) {
      if (!list.some((s) => s.id === id)) { es.close(); streams.delete(id); slots.delete(id); }
    }
    for (const s of list) {
      if (streams.has(s.id)) continue;
      const es = openSessionsStream(s);
      es.addEventListener('sessions', (e) => {
        slots.set(s.id, { sessions: JSON.parse((e as MessageEvent).data), error: null });
        recompute();
      });
      es.onerror = () => {
        slots.set(s.id, { sessions: slots.get(s.id)?.sessions ?? null, error: 'offline' });
        recompute();
      };
      streams.set(s.id, es);
    }
    recompute();
  }

  onMount(() => {
    servers = listServers();
    connect(servers);
    // O menu de conta (Sidebar) fica visivel com o quadro aberto, e remover um servidor NAO-ativo nao
    // recarrega a pagina. Sem reconciliar aqui: `servers` stale, EventSource orfao contra o server
    // removido e card apontando pra credencial que o usuario acabou de apagar.
    const off = onServersChanged(() => { servers = listServers(); connect(servers); });
    return () => { off(); for (const es of streams.values()) es.close(); streams.clear(); };
  });

  // Colunas fixas por estado; dentro, atividade recente primeiro (desempate por nome = estável).
  const COLS: { state: State; title: string }[] = [
    { state: 'awaiting_input', title: 'Precisa de você' },
    { state: 'working', title: 'Trabalhando' },
    { state: 'idle', title: 'Pronto' },
    { state: 'dead', title: 'Encerradas' },
  ];
  function byRecency(a: BoardRow, b: BoardRow): number {
    return (b.last_activity ?? 0) - (a.last_activity ?? 0) || a.name.localeCompare(b.name);
  }
  const cols = $derived(
    COLS.map((c) => ({ ...c, rows: rows.filter((r) => r.state === c.state).sort(byRecency) })),
  );

  // Coluna "encerrado" colapsada por padrão (persistida) — morta raramente interessa.
  let deadOpen = $state(localStorage.getItem('cp_board_dead') === '1');
  function toggleDead() {
    deadOpen = !deadOpen;
    localStorage.setItem('cp_board_dead', deadOpen ? '1' : '0');
  }

  // Rascunhos IÇADOS: o card troca de coluna (remonta) justamente quando o Claude termina —
  // o texto que você estava digitando não pode morrer com o card.
  const drafts = new Map<string, string>();
</script>

<div class="board">
  {#if offline.length}
    <p class="board-offline">sem conexão: {offline.join(', ')}</p>
  {/if}
  <div class="board-cols">
    {#each cols as col (col.state)}
      {#if col.state === 'dead' && !deadOpen}
        <!-- Encerradas colapsada = rail vertical de 56px: não gasta uma coluna de 320px com o que
             raramente interessa (é o que faz as outras 3 caberem sem scroll em ~1200px). -->
        <button class="dead-rail" onclick={toggleDead} aria-expanded="false"
                title="Mostrar sessões encerradas">
          <span class="rail-label">{col.title}</span>
          <span class="rail-count">{col.rows.length}</span>
        </button>
      {:else}
        <section class="board-col">
          <header class="col-head" style="--col-color: {stateColors[col.state]}">
            <span class="col-dot" class:pulse={col.state === 'awaiting_input'} aria-hidden="true"></span>
            <span class="col-title">{col.title}</span>
            <span class="col-count">{col.rows.length}</span>
            {#if col.state === 'dead'}
              <button class="col-collapse" onclick={toggleDead} aria-expanded="true"
                      title="Recolher">−</button>
            {/if}
          </header>
          <div class="col-cards">
            {#each col.rows as row (row.serverId + '::' + row.name)}
              <BoardCard
                session={row}
                server={servers.find((s) => s.id === row.serverId)!}
                color={serverColor(row.serverId)}
                draft={drafts.get(row.serverId + '::' + row.name) ?? ''}
                onDraftChange={(t) => drafts.set(row.serverId + '::' + row.name, t)}
                onOpen={() => onOpenSession(row.name, row.serverId)}
              />
            {/each}
            {#if col.rows.length === 0}
              <p class="col-empty">vazio</p>
            {/if}
          </div>
        </section>
      {/if}
    {/each}
  </div>
</div>

<style>
  /* Board: colunas de largura FIXA + scroll horizontal. 4×320 não cabe em 820px — abaixo de
     ~1200px o board scrolla em vez de espremer coluna (padrão de board com 4+ colunas). */
  .board { height: 100%; overflow-x: auto; overflow-y: hidden; padding: 24px; }
  .board-offline { color: var(--warning); font-size: var(--text-xs); margin: 0 0 var(--space-2); }
  .board-cols { display: flex; gap: 16px; height: 100%; align-items: stretch; }
  /* Coluna NÃO tem fundo próprio nem borda: em dark, tingir fundo por estado mata a hierarquia de
     luminância (o card é que é a superfície). Estado vive só no header. */
  .board-col { flex: 0 0 320px; display: flex; flex-direction: column; min-width: 0; }
  .col-head {
    display: flex; align-items: center; gap: var(--space-2);
    padding: 0 2px var(--space-2);
    border-bottom: 2px solid var(--col-color);   /* única marca de cor da coluna */
  }
  .col-dot { width: 8px; height: 8px; border-radius: 50%; background: var(--col-color); flex-shrink: 0; }
  /* A ÚNICA animação do board: o dot de "Precisa de você". Nada mais pulsa. */
  .col-dot.pulse { animation: dot-pulse 2s var(--ease-in-out) infinite; }
  @keyframes dot-pulse { 50% { opacity: 0.35; transform: scale(0.85); } }
  .col-title { font-size: var(--text-sm); font-weight: 510; color: var(--text-secondary); }
  .col-count { font-size: var(--text-sm); color: var(--text-muted); }
  /* min-* zerados: o alvo de toque global (button { min-height/min-width: 44px }) inflaria o header
     desta coluna pra 54px e desalinharia a borda dela das outras 3. Mesmo escape dos botoezinhos da
     Sidebar (.sess-del/.sess-git). Quadro e desktop-only, o alvo de 44px nao se aplica. */
  .col-collapse {
    margin-left: auto; width: 20px; height: 20px; min-height: 0; min-width: 0; flex-shrink: 0;
    background: none; border: 0; color: var(--text-muted); cursor: pointer; font-size: 14px; line-height: 1;
  }
  .col-cards { flex: 1; overflow-y: auto; padding: var(--space-2) 2px; display: flex; flex-direction: column; gap: var(--space-2); }
  .col-empty { color: var(--text-muted); font-size: var(--text-xs); text-align: center; padding: var(--space-4) 0; }
  /* Rail da coluna encerrada: 56px, label rotacionado, zero cor. */
  .dead-rail {
    flex: 0 0 56px; display: flex; flex-direction: column; align-items: center; gap: var(--space-2);
    padding: var(--space-3) 0; background: none; cursor: pointer;
    border: 1px solid var(--border-subtle); border-radius: var(--radius-md);
    color: var(--text-muted); opacity: 0.5;
  }
  .dead-rail:hover { opacity: 1; }
  .rail-label { writing-mode: vertical-rl; font-size: var(--text-xs); font-weight: 510; }
  .rail-count { font-size: var(--text-xs); }
</style>
