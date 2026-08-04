<script lang="ts">
  import BottomSheet from './BottomSheet.svelte';
  import { renderMarkdown } from '../lib/markdown';
  import { getSessions, pairSession, unpairSession, getHistory, getPairContract } from '../lib/api';
  import { stateLabels, stateColors, parsePeerMessage, relativeTime, encodeCompareIds } from '../lib/format';
  import { getActiveId } from '../lib/auth';
  import type { SessionInfo } from '../lib/types';

  interface Props {
    open: boolean;
    sessionName: string;              // sessão atual (um membro do grupo)
    pairPeers: string[] | null;       // os OUTROS membros do grupo, ou null
    onClose: () => void;
    onChanged: () => void;            // grupo mudou -> pai recarrega a lista (badge/chip)
    onOpenSplit?: (peer: string) => void; // desktop: abre o chat do membro lado a lado (split view)
    onOpenPeerChat?: (peer: string) => void; // abre o chat do membro num MODAL (as duas views)
  }
  let { open, sessionName, pairPeers, onClose, onChanged, onOpenSplit, onOpenPeerChat }: Props = $props();

  const peers = $derived(pairPeers ?? []);
  // Chave PRIMITIVA: a prop pairPeers é um array novo por referência a cada poll de 5s do pai —
  // o $effect de (re)carga dependendo do array resetava seleção/task/feed com o sheet ABERTO.
  const peersKey = $derived(peers.join(','));

  let sessions = $state<SessionInfo[]>([]);
  let picked = $state<string[]>([]);   // MULTI-select: marca N sessões e pareia de uma vez
  let task = $state('');

  function togglePick(name: string) {
    picked = picked.includes(name) ? picked.filter((n) => n !== name) : [...picked, name];
  }
  let busy = $state(false);
  let error = $state<string | null>(null);
  let adding = $state(false);   // grupo existente: mostrando o picker de "adicionar membro"

  // Timeline "conversa do grupo": recados [de: X] trocados entre os membros, garimpados dos
  // históricos de TODOS (user_msg com prefixo peer cujo remetente é outro membro) e fundidos por ts.
  type PeerMsg = { from: string; to: string; text: string; ts: number };
  let feed = $state<PeerMsg[]>([]);
  let feedLoading = $state(false);
  let feedError = $state<string | null>(null); // membros cujo histórico falhou (≠ conversa vazia)
  // Contrato compartilhado (markdown que os membros editam via fs): exibido cru, read-only.
  let contract = $state<{ path: string; content: string } | null>(null);

  // epoch: o BottomSheet mantem o componente MONTADO entre aberturas — abrir/fechar/reabrir rapido
  // (ou o grupo mudar entre aberturas) deixava resposta ANTIGA resolver depois e sobrescrever
  // feed/contrato/lista com dado stale.
  let epoch = 0;

  async function loadFeed(members: string[], my: number) {
    feedLoading = true;
    try {
      const all = [sessionName, ...members];
      // Falha de fetch ≠ conversa vazia: sem distinguir, o histórico de um membro sumia do feed
      // calado ("nenhuma troca" com mensagens existindo).
      const results = await Promise.all(all.map((n) =>
        getHistory(n).then((h) => ({ ok: true as const, h })).catch(() => ({ ok: false as const, h: [] }))));
      if (my !== epoch) return;
      const failed = all.filter((_, i) => !results[i].ok);
      feedError = failed.length ? `sem histórico de: ${failed.join(', ')}` : null;
      const names = new Set(all);
      const msgs: PeerMsg[] = [];
      results.forEach(({ h: evs }, i) => {
        const owner = all[i];
        for (const e of evs) {
          if (e.kind !== 'user_msg' || !e.text) continue;
          const p = parsePeerMessage(e.text);
          // Só recados vindos de OUTRO membro do grupo (ignora claude-pocket/terceiros).
          if (p && p.from !== owner && names.has(p.from)) {
            msgs.push({ from: p.from, to: owner, text: p.text, ts: e.ts ?? 0 });
          }
        }
      });
      feed = msgs.sort((a, b) => a.ts - b.ts).slice(-40); // cauda; histórico completo vive nos chats
    } finally {
      if (my === epoch) feedLoading = false;
    }
  }

  $effect(() => {
    if (!open) return;
    // Depende de open + peersKey (primitivos) — NUNCA do array peers: re-rodar por identidade
    // (poll de 5s) apagava seleção/task e refazia os fetches com o sheet aberto.
    const members = peersKey ? peersKey.split(',') : [];
    const my = ++epoch;
    picked = [];
    task = '';
    busy = false;
    error = null;
    adding = false;
    feed = [];
    feedError = null;
    contract = null;
    if (members.length) {
      loadFeed(members, my);
      getPairContract(sessionName)
        .then((c) => { if (my === epoch) contract = { path: c.path, content: c.content }; })
        .catch(() => { if (my === epoch) contract = null; });
    }
    getSessions()
      .then((all) => { if (my === epoch) sessions = all.filter((s) => s.name !== sessionName && s.state !== 'dead'); })
      .catch(() => { if (my === epoch) error = 'Não deu pra listar as sessões.'; });
  });

  // Candidatas a ENTRAR no grupo: vivas, fora do grupo atual (sessions completa fica pra stateOf).
  const candidates = $derived(sessions.filter((s) => !peers.includes(s.name)));

  async function doPair() {
    if (!picked.length || busy) return;
    busy = true;
    error = null;
    try {
      // Mesmo endpoint pra criar grupo e pra ADICIONAR membro (o backend une os grupos).
      const res = await pairSession(sessionName, picked, task.trim());
      onChanged();
      if (res.warning) {
        // Falha PARCIAL de aviso (membro sem o prompt): mostra em vez de fechar mudo.
        error = res.warning;
      } else {
        onClose();
      }
    } catch {
      error = `Falhou o pareamento com ${picked.join(', ')}.`;
    } finally {
      busy = false;
    }
  }

  async function doLeave() {
    if (busy) return;
    busy = true;
    error = null;
    try {
      const res = await unpairSession(sessionName);
      onChanged();
      if (res.warning) {
        error = res.warning;
      } else {
        onClose();
      }
    } catch {
      error = 'Falhou a saída do grupo.';
    } finally {
      busy = false;
    }
  }

  // Estado vivo de um membro (bolinha na linha), da lista já carregada pro picker.
  function stateOf(name: string): string | null {
    return sessions.find((s) => s.name === name)?.state ?? null;
  }

  // "Ver em grade": abre o GRUPO inteiro (eu + membros) na grade de comparação existente —
  // cards ao vivo (transcript/preview/estado), 1 clique entra na sessão. Grupo é sempre do
  // servidor ativo (pareamento é por servidor).
  function openGrid() {
    const sid = getActiveId();
    if (!sid) return;
    const ids = [sessionName, ...peers].map((name) => ({ serverId: sid, name }));
    onClose();
    window.location.hash = '#/compare/' + encodeCompareIds(ids);
  }

  // "Abrir todas lado a lado" (desktop): fixa cada membro num painel próprio do split.
  function openAllSplit() {
    if (!onOpenSplit) return;
    for (const p of peers) onOpenSplit(p);
    onClose();
  }
</script>

<!-- `resizable`: o painel do par carrega o contrato do grupo (um documento), a lista de membros e a
     conversa — nos 420px padrao o markdown saia com ~40 caracteres por linha. Fica arrastavel pela
     borda esquerda e a largura persiste. -->
<BottomSheet {open} {onClose} resizable widthKey="cp_pairsheet_w" defaultWidth={760} ariaLabel="Parear sessões">
  <div class="pair">
    {#if peers.length}
      <!-- Tudo que ROLA fica aqui; o rodape com "Sair do grupo" fica preso embaixo. -->
      <div class="pair-scroll">
      <h2 class="title">🤝 Grupo de trabalho ({peers.length + 1})</h2>
      <p class="hint">
        Os membros trabalham juntos: trocam contrato, avisos e dúvidas via cp-send por conta
        própria, cada um no seu repo. Sair avisa o grupo (os demais continuam entre si).
      </p>

      <!-- Membros: estado vivo + abrir lado a lado (desktop) por membro. -->
      <div class="list">
        {#each peers as p (p)}
          {@const st = stateOf(p)}
          <div class="row row--member">
            {#if st}<span class="dot" style="background: {stateColors[st as keyof typeof stateColors]};" aria-hidden="true"></span>{/if}
            <span class="row-main"><span class="row-name">{p}</span></span>
            {#if st}<span class="row-paired">{stateLabels[st as keyof typeof stateLabels]}</span>{/if}
            {#if onOpenPeerChat}
              <!-- Ícones em vez de glifos: `⤢` (expandir) e `⫽` (paralelas) não diziam o que fazem.
                   Aqui: um balão de conversa = abrir a conversa dele; duas colunas = lado a lado. -->
              <button class="split-btn" onclick={() => onOpenPeerChat?.(p)}
                      title={`Abrir a conversa de ${p}`} aria-label={`Abrir a sessão ${p} num modal`}>
                <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                     stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
                  <path d="M20 15a2 2 0 0 1-2 2H8l-4 3V6a2 2 0 0 1 2-2h12a2 2 0 0 1 2 2z"/>
                </svg>
              </button>
            {/if}
            {#if onOpenSplit}
              <button class="split-btn" onclick={() => onOpenSplit?.(p)}
                      title={`Abrir ${p} lado a lado`} aria-label={`Abrir ${p} lado a lado`}>
                <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                     stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
                  <rect x="3" y="4.5" width="8" height="15" rx="1.5"/>
                  <rect x="13" y="4.5" width="8" height="15" rx="1.5"/>
                </svg>
              </button>
            {/if}
          </div>
        {/each}
      </div>

      <!-- Visualizar o grupo inteiro: grade (cards ao vivo, 1 clique abre) ou split (N chats fixos). -->
      <div class="view-row">
        <button class="view-btn" onclick={openGrid} title="Grade com todos os membros ao vivo">▦ Ver em grade</button>
        {#if onOpenSplit && peers.length > 0}
          <button class="view-btn" onclick={openAllSplit} title="Fixa cada membro num painel lado a lado">⫽ Todas lado a lado</button>
        {/if}
      </div>

      {#if !adding}
        <button class="ghost-add" onclick={() => (adding = true)}>+ Adicionar sessão ao grupo</button>
      {:else}
        <div class="list">
          {#if candidates.length === 0}
            <p class="empty">Nenhuma outra sessão viva fora do grupo.</p>
          {:else}
            {#each candidates as s (s.name)}
              <button class="row" class:row--picked={picked.includes(s.name)}
                      onclick={() => togglePick(s.name)}
                      aria-label={`Adicionar ${s.name} ao grupo — ${stateLabels[s.state]}`}>
                <span class="dot" style="background: {stateColors[s.state]};" aria-hidden="true"></span>
                <span class="row-main">
                  <span class="row-name">{s.name}</span>
                  {#if s.cwd}<span class="row-cwd">{s.cwd}</span>{/if}
                </span>
                {#if s.pair_peers?.length}
                  <span class="row-paired" title={`Já agrupada com ${s.pair_peers.join(', ')}`}>🤝 {s.pair_peers.length}</span>
                {/if}
              </button>
            {/each}
          {/if}
        </div>
        <button class="primary-btn" onclick={doPair} disabled={!picked.length || busy}>
          {busy ? 'Adicionando…' : picked.length ? `Adicionar ${picked.join(', ')}` : 'Escolha sessões'}
        </button>
      {/if}

      {#if contract?.content}
        <!-- Contrato compartilhado: as sessões escrevem no arquivo; aqui só leitura. -->
        <div class="contract">
          <h3 class="feed-title">Contrato compartilhado</h3>
          <!-- Markdown RENDERIZADO, nao texto cru: o contrato e .md e ler "**Tarefa:**" com os
               asteriscos e pior em tudo. Regra do projeto (CLAUDE.md): todo .md exibido no app passa
               pelo renderMarkdown. -->
          <div class="contract-body md">{@html renderMarkdown(contract.content, { joinWrapped: true })}</div>
          <span class="contract-path" title={contract.path}>{contract.path}</span>
        </div>
      {/if}

      <!-- Conversa do grupo: o que os membros já combinaram, num lugar só. -->
      <div class="feed">
        <h3 class="feed-title">Conversa do grupo</h3>
        {#if feedError}
          <p class="empty">⚠ {feedError}</p>
        {/if}
        {#if feedLoading}
          <p class="empty">Carregando…</p>
        {:else if feed.length === 0}
          <p class="empty">Nenhuma troca entre os membros ainda.</p>
        {:else}
          {#each feed as m, i (i)}
            <div class="feed-item" class:feed-item--out={m.from === sessionName}>
              <span class="feed-meta">{m.from} → {m.to}{#if m.ts}&nbsp;· {relativeTime(m.ts)}{/if}</span>
              <span class="feed-text">{m.text}</span>
            </div>
          {/each}
        {/if}
      </div>
      </div><!-- /pair-scroll -->

      <!-- Rodape FIXO: "Sair do grupo" e destrutivo e nao pode fugir com o scroll do contrato. -->
      <div class="pair-foot">
        {#if error}<p class="error">{error}</p>{/if}
        <button class="danger-btn" onclick={doLeave} disabled={busy}>
          {busy ? 'Saindo…' : 'Sair do grupo'}
        </button>
      </div>
    {:else}
      <h2 class="title">Parear com sessão</h2>
      <p class="hint">
        Passam a trabalhar juntas: cada uma no seu repo, mandando o que a outra precisar
        (contrato, endpoint, aviso de conclusão) sem você intermediar. Escolher uma sessão já
        agrupada te coloca no grupo dela.
      </p>

      {#if error}<p class="error">{error}</p>{/if}

      <div class="list">
        {#if candidates.length === 0 && !error}
          <p class="empty">Nenhuma outra sessão viva.</p>
        {:else}
          {#each candidates as s (s.name)}
            <button class="row" class:row--picked={picked.includes(s.name)}
                    onclick={() => togglePick(s.name)}
                    aria-label={`Parear com ${s.name} — ${stateLabels[s.state]}`}>
              <span class="dot" style="background: {stateColors[s.state]};" aria-hidden="true"></span>
              <span class="row-main">
                <span class="row-name">{s.name}</span>
                {#if s.cwd}<span class="row-cwd">{s.cwd}</span>{/if}
              </span>
              {#if s.pair_peers?.length}
                <span class="row-paired" title={`Já agrupada com ${s.pair_peers.join(', ')}`}>🤝 {s.pair_peers.length}</span>
              {/if}
            </button>
          {/each}
        {/if}
      </div>

      <input
        type="text"
        class="task-input"
        bind:value={task}
        placeholder="Tarefa (opcional): ex. ABC-1234 — tela X + endpoint"
      />

      <button class="primary-btn" onclick={doPair} disabled={!picked.length || busy}>
        {busy ? 'Pareando…' : picked.length ? `Parear com ${picked.join(', ')}` : 'Escolha sessões (uma ou várias)'}
      </button>
    {/if}
  </div>
</BottomSheet>

<style>
  .pair { padding: var(--space-4); display: flex; flex-direction: column; gap: var(--space-3); }

  .title { font-size: var(--text-base); font-weight: 600; color: var(--text-primary); }

  .hint { font-size: var(--text-sm); color: var(--text-secondary); line-height: 1.5; }

  .error { font-size: var(--text-sm); color: #e5484d; }
  .empty { font-size: var(--text-sm); color: var(--text-muted); padding: var(--space-3) 0; }

  .list { display: flex; flex-direction: column; }

  .row {
    display: flex; align-items: center; gap: var(--space-3);
    width: 100%; padding: var(--space-3);
    background: none; border: 1px solid transparent; border-radius: var(--radius-md);
    text-align: left; cursor: pointer;
    -webkit-tap-highlight-color: transparent;
  }
  .row:hover { background: var(--bg-hover); }
  .row--picked { border-color: var(--accent); background: var(--accent-dim); }

  .dot { width: 8px; height: 8px; border-radius: 50%; flex-shrink: 0; }

  .row-main { display: flex; flex-direction: column; min-width: 0; flex: 1; }
  .row-name { font-size: var(--text-base); color: var(--text-primary); font-weight: 500; }
  .row-cwd {
    font-size: var(--text-xs); color: var(--text-muted);
    white-space: nowrap; overflow: hidden; text-overflow: ellipsis;
  }
  .row-paired { font-size: var(--text-xs); color: var(--text-muted); flex-shrink: 0; }

  /* Linha de MEMBRO do grupo (não clicável; ações à direita). */
  .row--member { cursor: default; }
  .row--member:hover { background: none; }

  /* Abrir membro lado a lado (desktop). */
  .split-btn {
    width: 30px; height: 30px; flex-shrink: 0;
    display: flex; align-items: center; justify-content: center;
    border: 1px solid var(--border-subtle); border-radius: var(--radius-sm);
    background: var(--surface-raised); color: var(--text-secondary);
    font-size: 13px; cursor: pointer;
  }
  .split-btn:hover { color: var(--text-primary); background: var(--bg-hover); }

  /* Botões de visualização do grupo (grade / lado a lado). */
  .view-row { display: flex; gap: var(--space-2); }
  .view-btn {
    flex: 1; height: 40px;
    border: 1px solid var(--border-default); border-radius: var(--radius-md);
    background: var(--surface-card); color: var(--text-primary);
    font-size: var(--text-sm); cursor: pointer;
  }
  .view-btn:hover { background: var(--bg-hover); }

  /* "+ Adicionar sessão ao grupo": discreto, abre o picker. */
  .ghost-add {
    width: 100%; height: 40px;
    border: 1px dashed var(--border-default); border-radius: var(--radius-md);
    background: none; color: var(--text-secondary);
    font-size: var(--text-sm); cursor: pointer;
  }
  .ghost-add:hover { color: var(--text-primary); background: var(--bg-hover); }

  .task-input {
    height: 44px;
    background: var(--surface-card);
    border: 1px solid var(--border-default);
    border-radius: var(--radius-md);
    color: var(--text-primary);
    font-family: var(--font-ui);
    font-size: 16px;
    padding: 0 var(--space-3);
    outline: none;
    width: 100%;
  }
  .task-input:focus { border-color: var(--accent); box-shadow: 0 0 0 2px var(--accent-dim); }
  .task-input::placeholder { color: var(--text-muted); }

  .primary-btn {
    width: 100%; height: 50px;
    background: var(--accent); border-radius: var(--radius-md);
    color: #fff; font-size: var(--text-base); font-weight: 600;
  }
  .primary-btn:disabled { opacity: 0.5; cursor: default; }

  /* Contrato compartilhado: box mono rolável, read-only. */
  .contract {
    display: flex; flex-direction: column; gap: var(--space-2);
    border-top: 1px solid var(--border-subtle);
    padding-top: var(--space-3);
  }
  /* Painel em coluna: cabecalho+conteudo rolam, rodape fica. `min-height: 0` e o que permite o
     filho encolher dentro do flex (sem ele o scroller cresce e empurra o rodape pra fora). */
  .pair { display: flex; flex-direction: column; height: 100%; min-height: 0; }
  .pair-scroll { flex: 1; min-height: 0; overflow-y: auto; overscroll-behavior: contain; }
  .pair-foot {
    flex-shrink: 0;
    padding-top: var(--space-3);
    margin-top: var(--space-2);
    border-top: 1px solid var(--border-subtle);
    background: transparent;
  }

  .contract-body {
    font-size: var(--text-xs);
    line-height: 1.55;
    color: var(--text-secondary);
    background: var(--surface-card);
    border: 1px solid var(--border-subtle);
    border-radius: var(--radius-md);
    padding: var(--space-3) var(--space-4);
    word-break: break-word;
    /* SEM teto de altura: o contrato inteiro corre no scroll do proprio painel. Com max-height
       virava caixa-dentro-de-caixa — duas barras de rolagem competindo e o fim do documento
       escondido atras da segunda. */
  }
  /* Tipografia do markdown do contrato. */
  .contract-body :global(h1) { margin: 0 0 var(--space-2); font-size: var(--text-sm); color: var(--text-primary); }
  .contract-body :global(h2) { margin: var(--space-3) 0 var(--space-1); font-size: var(--text-xs); text-transform: uppercase; letter-spacing: 0.06em; color: var(--text-muted); }
  .contract-body :global(h3) { margin: var(--space-2) 0 var(--space-1); font-size: var(--text-xs); color: var(--text-secondary); }
  .contract-body :global(p) { margin: 0 0 var(--space-2); }
  .contract-body :global(strong) { color: var(--text-primary); font-weight: 650; }
  .contract-body :global(ul), .contract-body :global(ol) { margin: 0 0 var(--space-2); padding-left: 1.2em; }
  .contract-body :global(li) { margin: 2px 0; }
  .contract-body :global(code) { padding: 0 4px; border-radius: 3px; background: var(--surface-raised); font-family: var(--font-mono); font-size: 11px; color: var(--text-primary); }
  .contract-body :global(pre) { margin: 0 0 var(--space-2); padding: var(--space-2); overflow-x: auto; border-radius: var(--radius-sm); background: var(--surface-inset); }
  /* O pre DENTRO do code-block (header novo) perde a caixa propria — o reset global do app.css
     perde em especificidade pra regra scoped acima, e sem isto virava borda dentro de borda. */
  .contract-body :global(.code-block pre) { background: none; border: none; border-radius: 0; margin: 0; }
  .contract-body :global(a) { color: var(--accent); }
  .contract-body :global(hr) { margin: var(--space-3) 0; border: 0; border-top: 1px solid var(--border-subtle); }
  .contract-path {
    font-size: var(--text-xs); color: var(--text-muted);
    white-space: nowrap; overflow: hidden; text-overflow: ellipsis;
  }

  /* Conversa do par: timeline compacta, rolável. */
  .feed {
    display: flex; flex-direction: column; gap: var(--space-2);
    max-height: 40vh; overflow-y: auto;
    border-top: 1px solid var(--border-subtle);
    padding-top: var(--space-3);
  }
  .feed-title { font-size: var(--text-sm); font-weight: 600; color: var(--text-secondary); }
  .feed-item {
    display: flex; flex-direction: column; gap: 2px;
    padding: var(--space-2) var(--space-3);
    background: var(--surface-card);
    border: 1px solid var(--border-subtle);
    border-radius: var(--radius-md);
  }
  .feed-item--out { border-color: var(--accent-dim); }
  .feed-meta { font-size: var(--text-xs); color: var(--text-muted); }
  .feed-text {
    font-size: var(--text-sm); color: var(--text-primary); line-height: 1.45;
    white-space: pre-wrap; word-break: break-word;
    display: -webkit-box; line-clamp: 4; -webkit-line-clamp: 4; -webkit-box-orient: vertical; overflow: hidden;
  }

  .danger-btn {
    width: 100%; height: 50px;
    background: none; border: 1px solid #e5484d; border-radius: var(--radius-md);
    color: #e5484d; font-size: var(--text-base); font-weight: 600;
  }
  .danger-btn:disabled { opacity: 0.5; cursor: default; }
</style>
