<script lang="ts">
  import BottomSheet from './BottomSheet.svelte';
  import { desktop } from '../lib/desktop.svelte';
import * as m from '../paraglide/messages';
  import ThemeToggle from './ThemeToggle.svelte';
  import BackgroundToggle from './BackgroundToggle.svelte';
  import { basename, relativeTime, rotuloEstado, stateColors } from '@hangar/core';
  import { listServers, listOwnServers, selectServer, serverColor, getActiveId, type Server } from '../lib/auth';
  import { searchTranscriptsForServer, askHistoryForServer, getSearchContextForServer, estaDesligado, type SearchHit } from '@hangar/core';
  import type { ChatEvent, SessionInfo, State } from '@hangar/core';
  import { renderMarkdown } from '../lib/markdown';

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
  // Servidores que falharam na última busca: sem isto a falha virava "Nenhum resultado".
  type Falha = { server: Server; motivo: string };
  let falhas = $state.raw<Falha[]>([]);
  // Servidores ainda sem resposta; os marcados fora do ar ganham rótulo próprio enquanto esperam.
  let pending = $state.raw<Server[]>([]);
  let foraDoAr = $state.raw<Set<string>>(new Set());
  let semServidores = $state(false);
  let searchSeq = 0;
  let searchTerm = '';

  const termosBusca = $derived([...new Set(query.trim().toLowerCase().split(/\s+/).filter(Boolean))]);
  const escapar = (s: string) => s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  function destacar(texto: string, termos: string[]): { t: string; hit: boolean }[] {
    if (!termos.length) return [{ t: texto, hit: false }];
    const re = new RegExp(`(${termos.map(escapar).join('|')})`, 'gi');
    return texto.split(re).filter(Boolean).map((t) => ({ t, hit: termos.includes(t.toLowerCase()) }));
  }

  // Um bloco por conversa, na ordem dos resultados (mais recente primeiro).
  const grupos = $derived.by(() => {
    const porConversa = new Map<string, Hit[]>();
    for (const h of results) {
      const k = `${h.serverId}/${h.session_id}`;
      porConversa.set(k, [...(porConversa.get(k) ?? []), h]);
    }
    return [...porConversa.entries()].map(([chave, hits]) => ({ chave, hits }));
  });
  const resumoBusca = $derived(
    `${results.length === 1 ? m.busca_um_trecho() : m.busca_n_trechos({ n: String(results.length) })} · ${
      grupos.length === 1 ? m.busca_uma_conversa() : m.busca_n_conversas({ n: String(grupos.length) })}`,
  );

  // Prévia: o trecho aberto mostra a mensagem inteira e as vizinhas, sem sair da busca.
  const chaveHit = (h: Hit) => `${h.serverId}/${h.project}/${h.session_id}/${h.event_id || h.line}`;
  // Chave repetida num {#each} derruba o componente e a busca congela em "Buscando…": servidor
  // com backend antigo ainda devolve a mesma mensagem duas vezes quando o transcript a repete.
  function semRepetidos(hits: Hit[]): Hit[] {
    const vistos = new Set<string>();
    return hits.filter((h) => {
      const k = chaveHit(h);
      if (vistos.has(k)) return false;
      vistos.add(k);
      return true;
    });
  }
  let aberto = $state<string | null>(null);
  let contexto = $state<ChatEvent[] | null>(null);
  let contextoErro = $state('');
  async function alternarPrevia(h: Hit) {
    const k = chaveHit(h);
    if (aberto === k) { aberto = null; return; }
    aberto = k;
    contexto = null;
    contextoErro = '';
    const srv = listServers().find((s) => s.id === h.serverId);
    if (!srv || !h.event_id) { contextoErro = m.busca_contexto_indisponivel(); return; }
    try {
      const evs = await getSearchContextForServer(srv, h.project, h.session_id, h.event_id);
      if (aberto === k) contexto = evs;
    } catch (e) {
      if (aberto === k) contextoErro = e instanceof Error ? e.message : m.busca_contexto_indisponivel();
    }
  }
  function abrirNoPonto(h: Hit) {
    selectServer(h.serverId);
    onClose();
    const seg = [h.serverId, h.project, h.session_id, ...(h.event_id ? [h.event_id] : [])];
    window.location.hash = '#/archive/' + seg.map(encodeURIComponent).join('/');
  }

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
        hits: semRepetidos(r.hits.map((h) => ({ ...h, serverId: srv.id, serverLabel: srv.label }))),
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
      mode = searchOnly ? 'search' : 'sessions';   // modo busca abre direto na busca
      query = '';
      results = [];
      falhas = []; aberto = null; pending = []; foraDoAr = new Set(); semServidores = false;
      askAnswer = null; askErr = ''; asking = false;
      activeIdx = 0;
      searchSeq++; // resposta de antes de fechar não entra na busca nova

      // espera o sheet montar/animar antes de focar
      focusSearch();
    }
  });
  // No toque, foco automático abre o teclado por cima da lista; só a busca pura o pede de cara.
  function focusSearch() {
    if (searchOnly || matchMedia('(pointer: fine)').matches) requestAnimationFrame(() => searchEl?.focus());
  }
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
    focusSearch();
  }

  // Busca de conteudo debounced (250ms): reage a query no modo "conversas". Cleanup cancela o timer
  // anterior a cada tecla (debounce) e ao trocar de modo/fechar.
  $effect(() => {
    if (mode !== 'search') return;
    const term = query.trim();
    clearTimeout(searchTimer);
    if (!term) {
      results = [];
      falhas = [];
      pending = [];
      foraDoAr = new Set();
      semServidores = false;
      searching = false;
      // Redigitar o mesmo termo antes do debounce reaproveitaria as respostas antigas.
      searchSeq++;
      return;
    }
    searching = true;
    searchTimer = setTimeout(() => runSearch(term), 250);
    return () => clearTimeout(searchTimer);
  });

  function runSearch(term: string) {
    // Fan-out: 1 chamada por servidor; cada resposta entra na lista assim que chega, pra um servidor
    // lento não segurar os resultados dos outros.
    const seq = ++searchSeq;
    searchTerm = term;
    const all = listOwnServers();   // a busca de transcrição é do servidor inteiro: convite a barra
    const active = getActiveId();
    // Os já marcados fora do ar saem por último: não atrasam o disparo dos outros.
    const offline = all.filter((s) => s.id !== active && estaDesligado(s.id));
    const servers = [...all.filter((s) => !offline.includes(s)), ...offline];
    foraDoAr = new Set(offline.map((s) => s.id));
    semServidores = all.length === 0;
    pending = servers;
    results = [];
    falhas = [];
    aberto = null;
    searching = servers.length > 0;
    for (const s of servers) consultar(s, term, seq);
  }

  function motivoFalha(e: unknown): string {
    if (e instanceof DOMException && e.name === 'TimeoutError') return m.busca_motivo_timeout();
    const msg = e instanceof Error ? e.message : String(e);
    return /^\d{3}$/.test(msg) ? m.busca_motivo_http({ status: msg }) : msg;
  }

  function consultar(s: Server, term: string, seq: number) {
    // Resposta velha: outra busca começou, a query mudou ou trocou de modo -> descarta.
    const valid = () => seq === searchSeq && term === query.trim() && mode === 'search';
    searchTranscriptsForServer(s, term)
      .then(
        (hits) => {
          if (!valid()) return;
          const tagged = hits.map((h) => ({ ...h, serverId: s.id, serverLabel: s.label }));
          results = semRepetidos([...results, ...tagged]).sort((a, b) => b.mtime - a.mtime); // mais recente primeiro
        },
        (e) => {
          if (!valid()) return;
          console.warn(`busca em ${s.label} falhou`, e);
          falhas = [...falhas, { server: s, motivo: motivoFalha(e) }];
        },
      )
      .finally(() => {
        if (!valid()) return;
        pending = pending.filter((p) => p.id !== s.id);
        searching = false;
      });
  }

  // Refaz só quem falhou, na mesma busca: as respostas que já chegaram ficam.
  function retentar() {
    if (searchTerm !== query.trim() || !falhas.length) return;
    const alvo = falhas.map((f) => f.server);
    falhas = [];
    pending = [...pending, ...alvo];
    for (const s of alvo) consultar(s, searchTerm, searchSeq);
  }

  const pendingLine = $derived.by(() => {
    const normais = pending.filter((s) => !foraDoAr.has(s.id)).map((s) => s.label);
    const fora = pending.filter((s) => foraDoAr.has(s.id)).map((s) => s.label);
    return [
      normais.length ? m.busca_aguardando({ servidores: normais.join(', ') }) : '',
      fora.length ? m.busca_tentando_fora({ servidores: fora.join(', ') }) : '',
    ].filter(Boolean).join(' · ');
  });

  // Uma região viva só, cujo texto muda: leitor de tela anuncia a troca, não um nó novo por estado.
  const statusText = $derived.by(() => {
    if (mode !== 'search' || !query.trim()) return '';
    if (semServidores) return m.busca_sem_servidores();
    if (searching) return m.switcher_buscando();
    const partes = [
      results.length ? resumoBusca
        : !falhas.length && !pending.length ? m.busca_nenhum_todas({ termos: termosBusca.join(', ') }) : '',
      pendingLine,
    ];
    return partes.filter(Boolean).join(' · ');
  });

  const multiServer = $derived(listServers().length > 1);

  // Nome curto da pasta pra exibir no meta do hit (ultimo segmento do cwd real; fallback = projeto).
  function folderShort(h: Hit): string {
    if (h.cwd) return basename(h.cwd);
    return h.project;
  }

  function abrirSessao(h: Hit) {
    if (!h.session_name) return;
    selectServer(h.serverId); // toda navegacao seguinte mira o servidor dono do hit
    onPick(h.session_name);   // Chat.pickSession fecha o sheet + navega
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

<BottomSheet {open} {onClose} ariaLabel={searchOnly ? m.lista_buscar() : m.sessao_trocar_de()} centered={desktop.atual}
             largura={mode === 'search' ? 760 : undefined}>
  <h2 class="sheet-title">{searchOnly ? m.lista_buscar() : m.lista_titulo()}</h2>

  <!-- Alterna entre trocar de sessao (vivas) e buscar conteudo em todas as conversas (feature #10).
       Escondido no modo "só busca" (a navegação abre direto na busca, sem sessão atual pra trocar). -->
  {#if !searchOnly}
  <div class="tabs" role="tablist">
    <button
      class="tab" class:tab--on={mode === 'sessions'}
      role="tab" aria-selected={mode === 'sessions'}
      onclick={() => setMode('sessions')}
    >{m.lista_titulo()}</button>
    <button
      class="tab" class:tab--on={mode === 'search'}
      role="tab" aria-selected={mode === 'search'}
      onclick={() => setMode('search')}
    >{m.lista_buscar()}</button>
  </div>
  {/if}

  <input
    type="text"
    class="search"
    bind:value={query}
    bind:this={searchEl}
    onkeydown={onKeydown}
    placeholder={mode === 'search' ? m.switcher_buscar_conversas() : m.switcher_buscar_sessao()}
    autocomplete="off"
    autocorrect="off"
    autocapitalize="off"
    spellcheck={false}
    aria-label={mode === 'search' ? m.switcher_buscar_conversas() : m.switcher_buscar_sessao()}
  />

  {#if mode === 'search'}
    {#if query.trim()}
      <!-- RAG lexical: pergunta em linguagem natural -> claude responde onde o assunto apareceu. -->
      <button class="ask-btn" onclick={askAI} disabled={asking}>
        {asking ? m.switcher_perguntando() : m.switcher_perguntar()}
      </button>
    {/if}
    {#if askErr}<p class="ask-err" role="alert">{askErr}</p>{/if}
    {#if askAnswer}
      <div class="ask-card">
        <p class="ask-answer">{askAnswer.answer}</p>
        {#each askAnswer.hits as h (chaveHit(h))}{@render trecho(h, true)}{/each}
      </div>
    {/if}
    <div class="list" aria-busy={searching || pending.length > 0}>
      {#each falhas as f (f.server.id)}
        <p class="ask-err" role="alert">{m.busca_servidor_falhou_motivo({ servidor: f.server.label, motivo: f.motivo })}</p>
      {/each}
      {#if falhas.length}<button class="retry-btn" onclick={retentar}>{m.busca_tentar_de_novo()}</button>{/if}
      <p class="busca-resumo" class:empty={!results.length && !!statusText} role="status">{statusText}</p>
      {#if !query.trim()}
        <p class="empty">{m.busca_digite_todas()}</p>
      {:else if !searching && results.length}
        {#each grupos as g (g.chave)}
          {@const h0 = g.hits[0]}
          <section class="grupo">
            <header class="grupo-cab">
              {#if multiServer}
                <span class="srv-dot" style="background: {serverColor(h0.serverId)};" aria-hidden="true"></span>
              {/if}
              <span class="grupo-nome">{h0.live && h0.session_name ? h0.session_name : folderShort(h0)}</span>
              <span class="hit-meta">
                {#if multiServer}<span>{h0.serverLabel}</span><span class="sep">·</span>{/if}
                {#if h0.live && h0.session_name && h0.session_name !== folderShort(h0)}<span class="hit-folder">{folderShort(h0)}</span><span class="sep">·</span>{/if}
                <span class:live={h0.live}>{h0.live ? m.switcher_ativa() : m.switcher_arquivo()}</span>
                {#if h0.mtime}<span class="sep">·</span><span>{relativeTime(h0.mtime)}</span>{/if}
              </span>
            </header>
            {#each g.hits as h (chaveHit(h))}{@render trecho(h, false)}{/each}
          </section>
        {/each}
      {/if}
    </div>
  {:else}
  <div class="list">
    {#if sorted.length === 0}
      <p class="empty">{m.busca_nenhuma_sessao()}</p>
    {:else}
      {#each sorted as s, i (s.name)}
        <button
          class="row"
          class:row--current={s.name === currentName}
          class:row--active={i === activeIdx}
          onclick={() => tap(s)}
          onmousemove={() => (activeIdx = i)}
          aria-label={`${s.name} — ${rotuloEstado(s.state)}`}
        >
          <span class="dot" style="background: {stateColors[s.state]};" aria-hidden="true"></span>
          <span class="row-main">
            <!-- Identidade primaria = nome da sessao (o mesmo do sidebar/lista); o cwd e a linha secundaria. -->
            <span class="row-name">{s.name}</span>
            {#if s.cwd}<span class="row-cwd">{s.cwd}</span>{/if}
          </span>
          {#if s.name === currentName}
            <span class="badge-current">{m.switcher_atual()}</span>
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
      <span class="row-name row-name--new">{m.sessao_nova()}</span>
    </button>
  </div>

  <p class="kbd-hint" aria-hidden="true">{m.busca_atalhos()}</p>
  {/if}

  <!-- No celular tema e fundo já moram em Aparência. -->
  {#if mode !== 'search' && desktop.atual}
    <div class="theme-row">
      <span class="theme-label">{m.config_tema_curto()}</span>
      <ThemeToggle />
    </div>
    <div class="theme-row">
      <span class="theme-label">{m.config_fundo_curto()}</span>
      <BackgroundToggle />
    </div>
  {/if}
</BottomSheet>

{#snippet trecho(h: Hit, comPasta: boolean)}
  {@const k = chaveHit(h)}
  <div class="hit" class:hit--aberto={aberto === k}>
    <button class="hit-btn" onclick={() => alternarPrevia(h)} aria-expanded={aberto === k}>
      <span class="hit-cab">
        <span class="hit-quem" class:hit-quem--voce={h.role === 'user'}>
          {h.role === 'user' ? m.busca_voce() : m.busca_assistente()}
        </span>
        {#if comPasta}<span class="hit-folder">{h.live && h.session_name ? h.session_name : folderShort(h)}</span>{/if}
        {#if h.ts}<span class="hit-quando">{relativeTime(h.ts)}</span>{/if}
      </span>
      <span class="hit-snippet">{#each destacar(h.line, termosBusca) as p, i (i)}{#if p.hit}<mark>{p.t}</mark>{:else}{p.t}{/if}{/each}</span>
    </button>
    {#if aberto === k}
      <div class="previa">
        {#if contextoErro}
          <p class="ask-err" role="alert">{contextoErro}</p>
        {:else if !contexto}
          <p class="empty" role="status">{m.busca_carregando_contexto()}</p>
        {:else}
          {#each contexto as ev (ev.id)}
            <div class="previa-msg" class:previa-msg--alvo={ev.id === h.event_id}>
              <span class="hit-quem" class:hit-quem--voce={ev.kind === 'user_msg'}>
                {ev.kind === 'user_msg' ? m.busca_voce() : m.busca_assistente()}
              </span>
              <div class="md">{@html renderMarkdown(ev.text ?? '')}</div>
            </div>
          {/each}
        {/if}
        <div class="previa-acoes">
          <button class="previa-btn" onclick={() => abrirNoPonto(h)}>{m.busca_abrir_ponto()}</button>
          {#if h.live && h.session_name}
            <button class="previa-btn previa-btn--sec" onclick={() => abrirSessao(h)}>{m.busca_abrir_sessao()}</button>
          {/if}
        </div>
      </div>
    {/if}
  </div>
{/snippet}

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
    background: var(--surface-card);
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

  /* Resultados da busca de conteúdo: um bloco por conversa, um trecho por mensagem casada. */
  .busca-resumo {
    font-size: var(--text-xs);
    color: var(--text-muted);
    padding: 0 var(--space-1) var(--space-1);
  }
  .grupo {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: var(--space-2) 0;
    border-top: 1px solid var(--border-subtle);
  }
  .grupo:first-of-type { border-top: 0; }
  .grupo-cab {
    display: flex;
    align-items: baseline;
    gap: var(--space-2);
    padding: 0 var(--space-2) var(--space-1);
    min-width: 0;
  }
  .grupo-nome {
    font-size: var(--text-sm);
    font-weight: 600;
    color: var(--text-primary);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    flex-shrink: 1;
  }
  .hit { border-radius: var(--radius-md); }
  .hit--aberto { background: var(--surface-inset); box-shadow: inset 0 0 0 1px var(--border-subtle); }
  .hit-btn {
    width: 100%;
    display: flex;
    flex-direction: column;
    align-items: stretch;
    gap: 3px;
    padding: var(--space-2);
    text-align: left;
    background: transparent;
    border-radius: var(--radius-md);
  }
  .hit-btn:hover { background: var(--bg-hover); }
  .hit-cab {
    display: flex;
    justify-content: flex-start;
    align-items: center;
    gap: var(--space-2);
    font-size: var(--text-xs);
    color: var(--text-muted);
    min-width: 0;
  }
  .hit-quem {
    font-size: var(--text-xs);
    font-weight: 600;
    color: var(--text-secondary);
  }
  .hit-quem--voce { color: var(--accent); }
  .hit-quando { margin-left: auto; flex-shrink: 0; }
  .hit-snippet {
    font-size: var(--text-sm);
    line-height: 1.45;
    color: var(--text-primary);
    display: -webkit-box;
    -webkit-line-clamp: 3;
    line-clamp: 3;
    -webkit-box-orient: vertical;
    overflow: hidden;
  }
  .hit-snippet mark {
    background: var(--accent-dim);
    color: var(--text-primary);
    border-radius: 3px;
    padding: 0 2px;
  }
  .previa {
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
    padding: var(--space-2) var(--space-3) var(--space-3);
  }
  .previa-msg {
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding-left: var(--space-3);
    border-left: 2px solid var(--border-subtle);
    font-size: var(--text-sm);
    color: var(--text-secondary);
  }
  .previa-msg--alvo { border-left-color: var(--accent); color: var(--text-primary); }
  .previa-acoes { display: flex; gap: var(--space-2); flex-wrap: wrap; }
  .previa-btn {
    height: 34px;
    padding: 0 var(--space-3);
    border-radius: var(--radius-md);
    font-size: var(--text-sm);
    font-weight: 600;
    background: var(--accent);
    color: #fff;
  }
  .previa-btn--sec { background: var(--bg-hover); color: var(--text-primary); }
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
    .tab, .ask-btn, .retry-btn, .previa-btn { min-height: 44px; }
  }

  .search {
    width: 100%;
    height: 44px;
    background: var(--surface-card);
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
    background: var(--surface-card);
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
  .retry-btn {
    align-self: flex-start; min-height: 32px; margin: var(--space-1) 0;
    padding: 0 var(--space-2); border: 1px solid var(--border-default); border-radius: var(--radius-md);
    background: transparent; color: var(--accent); font-size: var(--text-sm);
  }
  .retry-btn:active { background: var(--accent-dim); }
  .ask-card {
    margin-top: var(--space-3); padding: var(--space-3);
    background: var(--surface-card); border: 1px solid var(--border-subtle); border-radius: var(--radius-md);
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
