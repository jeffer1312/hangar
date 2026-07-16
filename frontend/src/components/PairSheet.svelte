<script lang="ts">
  import BottomSheet from './BottomSheet.svelte';
  import { getSessions, pairSession, unpairSession, getHistory, getPairContract } from '../lib/api';
  import { stateLabels, stateColors, parsePeerMessage, relativeTime } from '../lib/format';
  import type { SessionInfo } from '../lib/types';

  interface Props {
    open: boolean;
    sessionName: string;            // sessão atual (um dos lados do par)
    pairedWith: string | null;      // par atual, ou null
    onClose: () => void;
    onChanged: () => void;          // pareou/despareou -> pai recarrega a lista (badge/chip)
    onOpenSplit?: () => void;       // desktop: abre o chat do par lado a lado (split view)
  }
  let { open, sessionName, pairedWith, onClose, onChanged, onOpenSplit }: Props = $props();

  let sessions = $state<SessionInfo[]>([]);
  let picked = $state<string | null>(null);
  let task = $state('');
  let busy = $state(false);
  let error = $state<string | null>(null);

  // Timeline "conversa do par": recados [de: X] trocados entre as duas sessões, garimpados dos
  // DOIS históricos (user_msg com prefixo peer cujo remetente é o outro lado) e fundidos por ts.
  type PeerMsg = { from: string; to: string; text: string; ts: number };
  let feed = $state<PeerMsg[]>([]);
  let feedLoading = $state(false);
  // Contrato compartilhado (markdown que as duas editam via fs): exibido cru, read-only.
  let contract = $state<{ path: string; content: string } | null>(null);

  // epoch: o BottomSheet mantem o componente MONTADO entre aberturas — abrir/fechar/reabrir rapido
  // (ou trocar de par entre aberturas) deixava resposta ANTIGA resolver depois e sobrescrever
  // feed/contrato/lista com dado stale do par anterior.
  let epoch = 0;

  async function loadFeed(peer: string, my: number) {
    feedLoading = true;
    try {
      const [mine, theirs] = await Promise.all([
        getHistory(sessionName).catch(() => []),
        getHistory(peer).catch(() => []),
      ]);
      if (my !== epoch) return;
      const pick = (evs: typeof mine, owner: string, sender: string): PeerMsg[] =>
        evs.flatMap((e) => {
          if (e.kind !== 'user_msg' || !e.text) return [];
          const p = parsePeerMessage(e.text);
          // Só recados vindos do OUTRO lado do par (ignora claude-pocket/terceiros).
          return p && p.from === sender ? [{ from: sender, to: owner, text: p.text, ts: e.ts ?? 0 }] : [];
        });
      feed = [...pick(mine, sessionName, peer), ...pick(theirs, peer, sessionName)]
        .sort((a, b) => a.ts - b.ts)
        .slice(-40); // cauda: conversa recente; histórico completo vive nos chats
    } finally {
      if (my === epoch) feedLoading = false;
    }
  }

  $effect(() => {
    if (!open) return;
    const my = ++epoch;
    picked = null;
    task = '';
    busy = false;
    error = null;
    feed = [];
    contract = null;
    if (pairedWith) {
      loadFeed(pairedWith, my);
      getPairContract(sessionName)
        .then((c) => { if (my === epoch) contract = { path: c.path, content: c.content }; })
        .catch(() => { if (my === epoch) contract = null; });
    }
    getSessions()
      .then((all) => { if (my === epoch) sessions = all.filter((s) => s.name !== sessionName && s.state !== 'dead'); })
      .catch(() => { if (my === epoch) error = 'Não deu pra listar as sessões.'; });
  });

  async function doPair() {
    if (!picked || busy) return;
    busy = true;
    error = null;
    try {
      await pairSession(sessionName, picked, task.trim());
      onChanged();
      onClose();
    } catch {
      error = `Falhou o pareamento com ${picked}.`;
    } finally {
      busy = false;
    }
  }

  async function doUnpair() {
    if (busy) return;
    busy = true;
    error = null;
    try {
      await unpairSession(sessionName);
      onChanged();
      onClose();
    } catch {
      error = 'Falhou o despareamento.';
    } finally {
      busy = false;
    }
  }
</script>

<BottomSheet {open} {onClose} ariaLabel="Parear sessões">
  <div class="pair">
    {#if pairedWith}
      <h2 class="title">🤝 Pareada com {pairedWith}</h2>
      <p class="hint">
        As duas sessões trabalham juntas: trocam contrato, avisos e dúvidas via cp-send por conta
        própria, cada uma no seu repo. Desparear avisa as duas.
      </p>
      {#if onOpenSplit}
        <button class="primary-btn" onclick={onOpenSplit}>⫽ Abrir lado a lado</button>
      {/if}

      {#if contract?.content}
        <!-- Contrato compartilhado: as sessões escrevem no arquivo; aqui só leitura. -->
        <div class="contract">
          <h3 class="feed-title">Contrato compartilhado</h3>
          <pre class="contract-body">{contract.content}</pre>
          <span class="contract-path" title={contract.path}>{contract.path}</span>
        </div>
      {/if}

      <!-- Conversa do par: o que as duas já combinaram, num lugar só. -->
      <div class="feed">
        <h3 class="feed-title">Conversa do par</h3>
        {#if feedLoading}
          <p class="empty">Carregando…</p>
        {:else if feed.length === 0}
          <p class="empty">Nenhuma troca entre as duas ainda.</p>
        {:else}
          {#each feed as m, i (i)}
            <div class="feed-item" class:feed-item--out={m.from === sessionName}>
              <span class="feed-meta">{m.from} → {m.to}{#if m.ts}&nbsp;· {relativeTime(m.ts)}{/if}</span>
              <span class="feed-text">{m.text}</span>
            </div>
          {/each}
        {/if}
      </div>

      {#if error}<p class="error">{error}</p>{/if}
      <button class="danger-btn" onclick={doUnpair} disabled={busy}>
        {busy ? 'Despareando…' : 'Desparear'}
      </button>
    {:else}
      <h2 class="title">Parear com sessão</h2>
      <p class="hint">
        As duas passam a trabalhar juntas: cada uma no seu repo, mandando o que a outra precisar
        (contrato, endpoint, aviso de conclusão) sem você intermediar.
      </p>

      {#if error}<p class="error">{error}</p>{/if}

      <div class="list">
        {#if sessions.length === 0 && !error}
          <p class="empty">Nenhuma outra sessão viva.</p>
        {:else}
          {#each sessions as s (s.name)}
            <button class="row" class:row--picked={picked === s.name}
                    onclick={() => (picked = picked === s.name ? null : s.name)}
                    aria-label={`Parear com ${s.name} — ${stateLabels[s.state]}`}>
              <span class="dot" style="background: {stateColors[s.state]};" aria-hidden="true"></span>
              <span class="row-main">
                <span class="row-name">{s.name}</span>
                {#if s.cwd}<span class="row-cwd">{s.cwd}</span>{/if}
              </span>
              {#if s.paired_with}
                <span class="row-paired" title={`Já pareada com ${s.paired_with}`}>🤝 {s.paired_with}</span>
              {/if}
            </button>
          {/each}
        {/if}
      </div>

      <input
        type="text"
        class="task-input"
        bind:value={task}
        placeholder="Tarefa (opcional): ex. TICKET-000 — tela X + endpoint"
      />

      <button class="primary-btn" onclick={doPair} disabled={!picked || busy}>
        {busy ? 'Pareando…' : picked ? `Parear com ${picked}` : 'Escolha uma sessão'}
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

  .task-input {
    height: 44px;
    background: var(--bg-surface);
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
  .contract-body {
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    line-height: 1.5;
    color: var(--text-secondary);
    background: var(--bg-surface);
    border: 1px solid var(--border-subtle);
    border-radius: var(--radius-md);
    padding: var(--space-3);
    white-space: pre-wrap;
    word-break: break-word;
    max-height: 30vh;
    overflow-y: auto;
  }
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
    background: var(--bg-surface);
    border: 1px solid var(--border-subtle);
    border-radius: var(--radius-md);
  }
  .feed-item--out { border-color: var(--accent-dim); }
  .feed-meta { font-size: var(--text-xs); color: var(--text-muted); }
  .feed-text {
    font-size: var(--text-sm); color: var(--text-primary); line-height: 1.45;
    white-space: pre-wrap; word-break: break-word;
    display: -webkit-box; -webkit-line-clamp: 4; -webkit-box-orient: vertical; overflow: hidden;
  }

  .danger-btn {
    width: 100%; height: 50px;
    background: none; border: 1px solid #e5484d; border-radius: var(--radius-md);
    color: #e5484d; font-size: var(--text-base); font-weight: 600;
  }
  .danger-btn:disabled { opacity: 0.5; cursor: default; }
</style>
