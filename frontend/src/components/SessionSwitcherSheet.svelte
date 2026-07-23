<script lang="ts">
  import BottomSheet from './BottomSheet.svelte';
  import ThemeToggle from './ThemeToggle.svelte';
  import { relativeTime, stateLabels, stateColors } from '../lib/format';
  import { listServers, selectServer, serverColor, getActiveId } from '../lib/auth';
  import { searchTranscriptsForServer, askHistoryForServer, type SearchHit } from '../lib/api';
  import type { SessionInfo, State } from '../lib/types';

  // Troca de sessao sem voltar pra home. Dois modos: "sessoes" (lista das outras sessoes vivas +
  // "Nova sessão") e "conversas" (busca de CONTEUDO em todos os transcripts, vivos + arquivados,
  // fan-out por servidor — feature #10). O mesmo campo de texto serve os dois modos.
  interface Props {
    open: boolean;
    sessions: SessionInfo[];
    currentName: string;
    onPick: (name: string) => void;
    onNew: () => void;
    onClose: () => void;
    // Modo "só busca" (feature #10): entrada "Buscar conversas" da navegação da lista/sidebar. Abre
    // direto na busca de conteúdo cross-servidor, sem a aba "Sessões" (não há sessão atual pra trocar).
    searchOnly?: boolean;
  }
  let { open, sessions, currentName, onPick, onNew, onClose, searchOnly = false }: Props = $props();

  type Mode = 'sessions' | 'search';
  // Hit da busca + de qual servidor veio (o fan-out chama 1 por servidor e junta) -> tap rota pro dono.
  type Hit = SearchHit & { serverId: string; serverLabel: string };

  let mode = $state<Mode>('sessions');
  let query = $state('');
  let searchEl = $state<HTMLInputElement | null>(null);
  // Item destacado pra navegacao por teclado (setas): 0..sorted.length-1 = sessoes; sorted.length = "Nova sessao".
  let activeIdx = $state(0);

  // Estado da busca de conteudo (modo "conversas").
  let results = $state<Hit[]>([]);
  let searching = $state(false);
  let searchTimer: ReturnType<typeof setTimeout> | undefined;

  // "Perguntar" (RAG lexical): claude -p no backend responde onde o assunto apareceu.
  // v1 roda SO no servidor ativo (cross-server fica pra v2 — decisao anotada no contrato).
  let asking = $state(false);
  let askAnswer = $state<{ answer: string; hits: Hit[] } | null>(null);
  let askErr = $state('');
  async function askAI() {
    const q = query.trim();
    const srv = listServers().find((x) => x.id === getActiveId());
    if (!q || !srv || asking) return;
    asking = true; askErr = ''; askAnswer = null;
    try {
      const r = await askHistoryForServer(srv, q);
      askAnswer = {
        answer: r.answer,
        hits: r.hits.map((h) => ({ ...h, serverId: srv.id, serverLabel: srv.label })),
      };
    } catch (e) {
      askErr = e instanceof Error ? e.message.replace(/^\d+:\s*/, '') : 'falhou';
    } finally {
      asking = false;
    }
  }

  // Ao abrir: volta pro modo sessoes, limpa busca, foca o campo (o switcher e de teclado — Ctrl+K
  // abria com foco no body e digitar nao filtrava) e reseta o destaque.
  $effect(() => {
    if (open) {
      mode = searchOnly ? 'search' : 'sessions';   // "Buscar conversas" abre direto na busca
      query = '';
      results = [];
      askAnswer = null; askErr = ''; asking = false;
      activeIdx = 0;
      // espera o sheet montar/animar antes de focar
      requestAnimationFrame(() => searchEl?.focus());
    }
  });
  // Digitar refiltra -> o destaque volta pro topo pra nunca apontar pra um item fora da lista.
  $effect(() => {
    query;
    activeIdx = 0;
  });

  // Troca de modo: limpa o campo/resultados e refoca (cada modo tem semantica de busca diferente).
  function setMode(m: Mode) {
    if (m === mode) return;
    mode = m;
    query = '';
    results = [];
    askAnswer = null; askErr = ''; asking = false;
    activeIdx = 0;
    requestAnimationFrame(() => searchEl?.focus());
  }

  // Busca de conteudo debounced (250ms): reage a query no modo "conversas". Cleanup cancela o timer
  // anterior a cada tecla (debounce) e ao trocar de modo/fechar.
  $effect(() => {
    if (mode !== 'search') return;
    const term = query.trim();
    clearTimeout(searchTimer);
    if (!term) {
      results = [];
      searching = false;
      return;
    }
    searching = true;
    searchTimer = setTimeout(() => runSearch(term), 250);
    return () => clearTimeout(searchTimer);
  });

  async function runSearch(term: string) {
    // Fan-out: 1 chamada por servidor (mesmo padrao de fetchSessionsForServer); um server lento/offline
    // falha isolado (allSettled) sem segurar os outros.
    const servers = listServers();
    const settled = await Promise.allSettled(servers.map((s) => searchTranscriptsForServer(s, term)));
    // Resultado velho: a query mudou (ou trocou de modo) enquanto o fetch voltava -> descarta.
    if (term !== query.trim() || mode !== 'search') return;
    const merged: Hit[] = [];
    settled.forEach((r, i) => {
      if (r.status === 'fulfilled') {
        for (const h of r.value) merged.push({ ...h, serverId: servers[i].id, serverLabel: servers[i].label });
      }
    });
    merged.sort((a, b) => b.mtime - a.mtime); // mais recente primeiro
    results = merged;
    searching = false;
  }

  const multiServer = $derived(listServers().length > 1);

  // Nome curto da pasta pra exibir no meta do hit (ultimo segmento do cwd real; fallback = projeto).
  function folderShort(h: Hit): string {
    if (h.cwd) return h.cwd.split('/').filter(Boolean).pop() ?? h.cwd;
    return h.project;
  }

  function tapHit(h: Hit) {
    selectServer(h.serverId); // toda navegacao seguinte mira o servidor dono do hit
    if (h.live && h.session_name) {
      onPick(h.session_name); // sessao viva -> abre o chat (Chat.pickSession fecha o sheet + navega)
    } else {
      // Conversa morta -> abre o leitor do Arquivo naquele transcript (deep-link no hash; o App rota).
      onClose();
      const seg = [h.serverId, h.project, h.session_id].map(encodeURIComponent).join('/');
      window.location.hash = '#/archive/' + seg;
    }
  }

  const urgency: Record<State, number> = {
    awaiting_input: 0,
    working: 1,
    idle: 2,
    dead: 3,
  };


  // Ordena por atividade (desc) + urgencia; aplica busca por nome/cwd.
  const sorted = $derived.by(() => {
    const q = query.trim().toLowerCase();
    return [...sessions]
      .sort((a, b) => {
        const byAct = (b.last_activity ?? 0) - (a.last_activity ?? 0);
        if (byAct !== 0) return byAct;
        return urgency[a.state] - urgency[b.state];
      })
      .filter(
        (s) => !q || s.name.toLowerCase().includes(q) || (s.cwd ?? '').toLowerCase().includes(q),
      );
  });

  // Total navegavel = sessoes filtradas + a linha "Nova sessao".
  const itemCount = $derived(sorted.length + 1);

  function tap(s: SessionInfo) {
    if (s.name === currentName) {
      onClose();
      return;
    }
    onPick(s.name);
  }

  // Setas movem o destaque (com wrap); Enter aciona o item destacado (sessao ou "Nova sessao").
  // So no modo "sessoes" — no modo "conversas" o campo e uma busca livre (sem nav por teclado).
  function onKeydown(e: KeyboardEvent) {
    if (mode !== 'sessions') return;
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      activeIdx = (activeIdx + 1) % itemCount;
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      activeIdx = (activeIdx - 1 + itemCount) % itemCount;
    } else if (e.key === 'Enter') {
      e.preventDefault();
      if (activeIdx >= sorted.length) onNew();
      else if (sorted[activeIdx]) tap(sorted[activeIdx]);
    }
  }
</script>

<BottomSheet {open} {onClose} ariaLabel={searchOnly ? 'Buscar conversas' : 'Trocar de sessão'}>
  <h2 class="sheet-title">{searchOnly ? 'Buscar conversas' : 'Sessões'}</h2>

  <!-- Alterna entre trocar de sessao (vivas) e buscar conteudo em todas as conversas (feature #10).
       Escondido no modo "só busca" (a navegação abre direto na busca, sem sessão atual pra trocar). -->
  {#if !searchOnly}
  <div class="tabs" role="tablist">
    <button
      class="tab" class:tab--on={mode === 'sessions'}
      role="tab" aria-selected={mode === 'sessions'}
      onclick={() => setMode('sessions')}
    >Sessões</button>
    <button
      class="tab" class:tab--on={mode === 'search'}
      role="tab" aria-selected={mode === 'search'}
      onclick={() => setMode('search')}
    >Buscar conversas</button>
  </div>
  {/if}

  <input
    type="text"
    class="search"
    bind:value={query}
    bind:this={searchEl}
    onkeydown={onKeydown}
    placeholder={mode === 'search' ? 'Buscar nas conversas' : 'Buscar sessão'}
    autocomplete="off"
    autocorrect="off"
    autocapitalize="off"
    spellcheck={false}
    aria-label={mode === 'search' ? 'Buscar nas conversas' : 'Buscar sessão'}
  />

  {#if mode === 'search'}
    {#if query.trim()}
      <!-- RAG lexical: pergunta em linguagem natural -> claude responde onde o assunto apareceu. -->
      <button class="ask-btn" onclick={askAI} disabled={asking}>
        {asking ? 'Perguntando…' : '✦ Perguntar: onde falei sobre isso?'}
      </button>
    {/if}
    {#if askErr}<p class="ask-err" role="alert">{askErr}</p>{/if}
    {#if askAnswer}
      <div class="ask-card">
        <p class="ask-answer">{askAnswer.answer}</p>
        {#each askAnswer.hits as h (h.project + '/' + h.session_id + '/' + h.line)}
          <button class="row row--hit" onclick={() => tapHit(h)}>
            <span class="row-main">
              <span class="hit-snippet">{h.line}</span>
              <span class="hit-meta">
                <span class="hit-folder">{folderShort(h)}</span>
                <span class="sep">·</span>
                <span class:live={h.live}>{h.live ? 'ativa' : 'arquivo'}</span>
              </span>
            </span>
            <span class="chev" aria-hidden="true">›</span>
          </button>
        {/each}
      </div>
    {/if}
    <div class="list">
      {#if !query.trim()}
        <p class="empty">Digite para buscar em todas as conversas.</p>
      {:else if searching}
        <p class="empty">Buscando…</p>
      {:else if results.length === 0}
        <p class="empty">Nenhum resultado.</p>
      {:else}
        {#each results as h (h.serverId + '/' + h.project + '/' + h.session_id)}
          <button class="row row--hit" onclick={() => tapHit(h)}>
            <span class="row-main">
              <span class="hit-snippet">{h.line}</span>
              <span class="hit-meta">
                {#if multiServer}
                  <span class="srv-dot" style="background: {serverColor(h.serverId)};" aria-hidden="true"></span>
                  <span class="srv-label">{h.serverLabel}</span>
                  <span class="sep">·</span>
                {/if}
                <span class="hit-folder">{folderShort(h)}</span>
                <span class="sep">·</span>
                <span class:live={h.live}>{h.live ? 'ativa' : 'arquivo'}</span>
                {#if h.mtime}<span class="sep">·</span><span>{relativeTime(h.mtime)}</span>{/if}
              </span>
            </span>
            <span class="chev" aria-hidden="true">›</span>
          </button>
        {/each}
      {/if}
    </div>
  {:else}
  <div class="list">
    {#if sorted.length === 0}
      <p class="empty">Nenhuma sessão encontrada.</p>
    {:else}
      {#each sorted as s, i (s.name)}
        <button
          class="row"
          class:row--current={s.name === currentName}
          class:row--active={i === activeIdx}
          onclick={() => tap(s)}
          onmousemove={() => (activeIdx = i)}
          aria-label={`${s.name} — ${stateLabels[s.state]}`}
        >
          <span class="dot" style="background: {stateColors[s.state]};" aria-hidden="true"></span>
          <span class="row-main">
            <!-- Identidade primaria = nome da sessao (o mesmo do sidebar/lista); o cwd e a linha secundaria. -->
            <span class="row-name">{s.name}</span>
            {#if s.cwd}<span class="row-cwd">{s.cwd}</span>{/if}
          </span>
          {#if s.name === currentName}
            <span class="badge-current">atual</span>
          {:else if s.last_activity}
            <span class="row-time">{relativeTime(s.last_activity)}</span>
          {/if}
        </button>
      {/each}
    {/if}

    <button
      class="row row--new"
      class:row--active={activeIdx >= sorted.length}
      onclick={onNew}
      onmousemove={() => (activeIdx = sorted.length)}
    >
      <span class="plus" aria-hidden="true">+</span>
      <span class="row-name row-name--new">Nova sessão</span>
    </button>
  </div>

  <p class="kbd-hint" aria-hidden="true">↑↓ mover · Enter abrir · Esc fechar</p>
  {/if}

  <div class="theme-row">
    <span class="theme-label">Tema</span>
    <ThemeToggle />
  </div>
</BottomSheet>

<style>
  .sheet-title {
    font-size: var(--text-xl);
    font-weight: 600;
    color: var(--text-primary);
    margin-bottom: var(--space-3);
  }

  /* Segmented control sessoes/conversas */
  .tabs {
    display: flex;
    gap: var(--space-1);
    background: var(--bg-surface);
    border: 1px solid var(--border-subtle);
    border-radius: var(--radius-md);
    padding: 3px;
    margin-bottom: var(--space-3);
  }
  .tab {
    flex: 1;
    height: 34px;
    border-radius: calc(var(--radius-md) - 3px);
    font-size: var(--text-sm);
    font-weight: 600;
    color: var(--text-secondary);
    background: transparent;
    transition: background 160ms var(--ease-out), color 160ms var(--ease-out);
  }
  .tab--on {
    color: var(--text-primary);
    background: var(--bg-hover);
    box-shadow: inset 0 0 0 1px var(--border-default);
  }

  /* Linha de resultado da busca de conteudo */
  .row--hit {
    align-items: flex-start;
    padding-top: var(--space-3);
    padding-bottom: var(--space-3);
  }
  .hit-snippet {
    font-size: var(--text-sm);
    color: var(--text-primary);
    display: -webkit-box;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    -webkit-box-orient: vertical;
    overflow: hidden;
  }
  .hit-meta {
    display: flex;
    align-items: center;
    gap: 5px;
    font-size: var(--text-xs);
    color: var(--text-muted);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .hit-folder {
    font-family: var(--font-mono);
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .hit-meta .sep { opacity: 0.5; }
  .hit-meta .live { color: var(--success); font-weight: 600; }
  .srv-dot {
    width: 7px;
    height: 7px;
    border-radius: var(--radius-full);
    flex-shrink: 0;
  }
  .chev {
    color: var(--text-muted);
    flex-shrink: 0;
    align-self: center;
  }

  .theme-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-top: var(--space-4);
    padding-top: var(--space-3);
    border-top: 1px solid var(--border-subtle);
  }
  .theme-label {
    font-size: var(--text-sm);
    color: var(--text-secondary);
  }

  /* Dica de teclado (desktop): torna os atalhos ↑↓/Enter/Esc do switcher descobriveis. Escondida em
     ponteiro coarse (toque), onde nao ha teclado e a dica so ocuparia espaco. */
  .kbd-hint {
    font-size: var(--text-xs);
    color: var(--text-muted);
    text-align: center;
    margin-top: var(--space-3);
  }
  @media (pointer: coarse) {
    .kbd-hint { display: none; }
  }

  .search {
    width: 100%;
    height: 44px;
    background: var(--bg-surface);
    border: 1px solid var(--border-default);
    border-radius: var(--radius-md);
    color: var(--text-primary);
    font-family: var(--font-ui);
    font-size: 16px; /* evita zoom no iOS */
    padding: 0 var(--space-3);
    outline: none;
    margin-bottom: var(--space-4);
    transition: border-color 180ms var(--ease-out);
  }
  .search::placeholder {
    color: var(--text-muted);
  }
  .search:focus {
    border-color: var(--accent);
    box-shadow: 0 0 0 2px var(--accent-dim);
  }

  .list {
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
    max-height: 56vh;
    overflow-y: auto;
    -webkit-overflow-scrolling: touch;
  }

  .row {
    width: 100%;
    min-height: 56px;
    display: flex;
    align-items: center;
    gap: var(--space-3);
    padding: var(--space-2) var(--space-3);
    border-radius: var(--radius-md);
    text-align: left;
    background: transparent;
    transition: background 160ms var(--ease-out);
  }
  .row:active {
    background: var(--bg-hover);
  }
  /* Item destacado por teclado (setas): mesmo realce do hover. */
  .row--active {
    background: var(--bg-hover);
    box-shadow: inset 0 0 0 1px var(--border-default);
  }
  .row--current {
    background: var(--bg-surface);
  }

  .dot {
    width: 8px;
    height: 8px;
    border-radius: var(--radius-full);
    flex-shrink: 0;
  }

  .row-main {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
    flex: 1;
  }

  .row-name {
    font-size: var(--text-base);
    font-weight: 600;
    color: var(--text-primary);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .row-cwd {
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    color: var(--text-muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .row-time {
    flex-shrink: 0;
    font-size: var(--text-xs);
    color: var(--text-muted);
  }

  .badge-current {
    flex-shrink: 0;
    font-size: 10px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    padding: 2px 7px;
    border-radius: var(--radius-full);
    color: var(--accent);
    background: var(--accent-dim);
  }

  /* Linha "Nova sessão" */
  .row--new {
    margin-top: var(--space-1);
    border-top: 1px solid var(--border-subtle);
    border-radius: 0;
    padding-top: var(--space-3);
  }

  .plus {
    width: 8px;
    text-align: center;
    font-size: var(--text-lg);
    font-weight: 600;
    color: var(--accent);
    flex-shrink: 0;
  }

  .row-name--new {
    color: var(--accent);
  }

  /* "Perguntar" (RAG lexical): botao discreto sob o campo + card de resposta acima dos hits. */
  .ask-btn {
    width: 100%; min-height: 38px; margin-top: var(--space-2);
    border: 1px dashed var(--border-default); border-radius: var(--radius-md);
    color: var(--accent); font-size: var(--text-sm); font-weight: 500;
    background: transparent;
  }
  .ask-btn:active:not(:disabled) { background: var(--accent-dim); }
  .ask-btn:disabled { opacity: 0.6; }
  .ask-err { margin: var(--space-2) 0 0; color: var(--error); font-size: var(--text-sm); }
  .ask-card {
    margin-top: var(--space-3); padding: var(--space-3);
    background: var(--bg-surface); border: 1px solid var(--border-subtle); border-radius: var(--radius-md);
  }
  .ask-answer {
    margin: 0 0 var(--space-2); color: var(--text-primary);
    font-size: var(--text-sm); line-height: 1.5; white-space: pre-wrap;
  }

  .empty {
    font-size: var(--text-sm);
    color: var(--text-muted);
    text-align: center;
    padding: var(--space-4) 0;
  }
</style>
