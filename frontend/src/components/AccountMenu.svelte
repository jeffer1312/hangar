<script lang="ts">
  import { enablePush, pushSupported } from '../lib/push';
  import { getPushSettings, setQuietHours } from '../lib/api';
  import { serverColor } from '../lib/auth';
  import type { Server } from '../lib/auth';

  // Menu de CONTA (estilo Claude): tudo que é conta/config vive aqui, fora da navegação. Popover que
  // abre pra CIMA a partir do avatar no rodapé — mesma peça no mobile (SessionList) e no desktop
  // (Sidebar), diferindo só pelos handlers que o pai passa (trocar servidor só existe no desktop).
  // Ações self-contained (push, horas silenciosas, renomear servidor) o menu resolve sozinho via libs;
  // as que dependem de UI do pai (adicionar/remover servidor, reconectar, sair) vêm por callback.
  interface Props {
    open: boolean;
    onClose: () => void;
    initials: string;
    accountName: string;
    accountSub?: string | null;
    servers: Server[];
    // Elemento âncora (o botão/avatar do rodapé) — usado pra posicionar o popover que abre pra cima.
    anchorEl?: HTMLElement | null;
    // embedded: renderiza o MESMO corpo inline (sem portal/backdrop/posição fixed), pro drawer do
    // mobile. O head (avatar) some — o drawer já tem o seu. Desktop/mobile-footer seguem como popover.
    embedded?: boolean;
    // Servidor ATIVO (desktop) — destaca + habilita a troca. null no mobile (lista agregada, sem "ativo").
    activeId?: string | null;
    onSwitchServer?: (id: string) => void;   // só desktop (troca + reload)
    onRenameServer: (id: string, label: string) => void;
    onRemoveServer: (id: string) => void;
    onAddServer: () => void;
    onReconnect: () => void;
    onLogout: () => void;
  }
  let {
    open, onClose, initials, accountName, accountSub = null, servers, anchorEl = null, embedded = false, activeId = null,
    onSwitchServer, onRenameServer, onRemoveServer, onAddServer, onReconnect, onLogout,
  }: Props = $props();

  // Portal pro <body>: a sidebar tem `backdrop-filter` (liquid glass no Chromium), que vira bloco de
  // contenção e RECORTA até `position: fixed` — o menu (e o backdrop) ficavam presos dentro dela. Mover
  // os nós pro body escapa qualquer ancestral filtrado/transformado.
  function portal(node: HTMLElement) {
    document.body.appendChild(node);
    return { destroy() { node.remove(); } };
  }

  // Web push: liga notificação de "sessão aguardando" (assina + registra nos servidores). Reusa o
  // MESMO enablePush das duas telas — não reimplementa.
  let pushBusy = $state(false);
  let pushMsg = $state('');
  async function handleEnablePush() {
    pushBusy = true;
    pushMsg = '';
    try {
      const n = await enablePush();
      pushMsg = `Ativado em ${n} servidor${n > 1 ? 'es' : ''}.`;
    } catch (e) {
      pushMsg = e instanceof Error ? e.message : 'Erro ao ativar.';
    } finally {
      pushBusy = false;
    }
  }

  // Horas silenciosas (feature #5): janela GLOBAL de silêncio pro push, do servidor ativo. <input
  // type="time"> nativo. Carrega ao abrir o menu; best-effort (offline/sem rota -> campos vazios).
  let qhStart = $state('');
  let qhEnd = $state('');
  let qhMsg = $state('');
  async function loadQuietHours() {
    try {
      const p = await getPushSettings();
      qhStart = p.quiet_hours?.start ?? '';
      qhEnd = p.quiet_hours?.end ?? '';
    } catch { /* offline/sem rota: campos ficam vazios, tenta salvar de novo depois */ }
    qhMsg = '';
  }
  async function saveQuietHours() {
    try {
      await setQuietHours(qhStart || null, qhEnd || null);
      qhMsg = qhStart && qhEnd ? `silenciado ${qhStart}–${qhEnd}` : 'desligado';
    } catch (e) {
      qhMsg = e instanceof Error ? e.message : 'erro ao salvar';
    }
  }
  // Recarrega a janela de silêncio toda vez que o menu abre (pode ter mudado no servidor).
  $effect(() => { if (open && pushSupported()) void loadQuietHours(); });

  // Rename inline de servidor: id em edição + valor do input. O pai persiste (renameServer + reagrega).
  let editingId = $state<string | null>(null);
  let editLabel = $state('');
  function startRename(id: string, current: string) {
    editingId = id;
    editLabel = current;
  }
  function saveRename() {
    if (editingId) onRenameServer(editingId, editLabel);
    editingId = null;
  }

  // Posição do card: FIXED, medida da âncora (rodapé) via getBoundingClientRect. Abre pra cima.
  let pos = $state({ left: 0, bottom: 0 });
  $effect(() => {
    if (!open || !anchorEl) return;
    const r = anchorEl.getBoundingClientRect();
    pos = { left: r.left, bottom: window.innerHeight - r.top + 8 }; // 8px acima do avatar
  });

  // Ações que disparam UI/fluxo do pai fecham o menu antes (o pai abre seu sheet/confirm/reload).
  function addServer() { onClose(); onAddServer(); }
  function reconnect() { onClose(); onReconnect(); }
  function logout() { onClose(); onLogout(); }
  function switchServer(id: string) {
    if (!onSwitchServer) return;
    onClose();
    onSwitchServer(id);
  }
</script>

<!-- Corpo do menu (servidores → sair): reusado igual no popover e no drawer embedded (uma só fonte
     de verdade pros handlers de push/quiet/rename/reconnect/logout). -->
{#snippet menuBody()}
    <div class="am-section">Servidores</div>
    {#each servers as s (s.id)}
      <div class="am-srv" class:on={s.id === activeId}>
        {#if editingId === s.id}
          <span class="am-dot" style="background: {serverColor(s.id)};" aria-hidden="true"></span>
          <!-- svelte-ignore a11y_autofocus -->
          <input
            class="am-srv-edit"
            bind:value={editLabel}
            onclick={(e) => e.stopPropagation()}
            onkeydown={(e) => { if (e.key === 'Enter') saveRename(); if (e.key === 'Escape') editingId = null; }}
            onblur={saveRename}
            autofocus
            aria-label="Novo nome do servidor"
          />
        {:else if onSwitchServer}
          <button class="am-srv-pick" onclick={() => switchServer(s.id)}>
            <span class="am-dot" style="background: {serverColor(s.id)};" aria-hidden="true"></span>
            <span class="am-srv-label">{s.label}</span>
            {#if s.id === activeId}<span class="am-tag">ativo</span>{/if}
          </button>
          <button class="am-srv-rename" onclick={() => startRename(s.id, s.label)} aria-label={`Renomear ${s.label}`} title="Renomear">✎</button>
          {#if servers.length > 1}
            <button class="am-srv-del" onclick={() => onRemoveServer(s.id)} aria-label={`Remover ${s.label}`}>×</button>
          {/if}
        {:else}
          <span class="am-dot" style="background: {serverColor(s.id)};" aria-hidden="true"></span>
          <span class="am-srv-label">{s.label}</span>
          <button class="am-srv-rename" onclick={() => startRename(s.id, s.label)} aria-label={`Renomear ${s.label}`} title="Renomear">✎</button>
          <button class="am-srv-del" onclick={() => onRemoveServer(s.id)} aria-label={`Remover ${s.label}`}>×</button>
        {/if}
      </div>
    {/each}
    <button class="am-item" role="menuitem" onclick={addServer}>
      <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" aria-hidden="true"><path d="M12 5v14M5 12h14"/></svg>
      Adicionar servidor
    </button>

    {#if pushSupported()}
      <div class="am-sep"></div>
      <button class="am-item" role="menuitem" onclick={handleEnablePush} disabled={pushBusy}>
        <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M6 8a6 6 0 0 1 12 0c0 7 3 9 3 9H3s3-2 3-9"/><path d="M10.3 21a1.94 1.94 0 0 0 3.4 0"/></svg>
        {pushBusy ? 'Ativando…' : 'Ativar notificações'}
      </button>
      {#if pushMsg}<div class="am-msg">{pushMsg}</div>{/if}
      <div class="am-quiet">
        <div class="am-quiet-head">
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M12 3a6 6 0 0 0 9 9 9 9 0 1 1-9-9z"/></svg>
          <span>Horas silenciosas</span>
        </div>
        <div class="am-quiet-row">
          <input type="time" bind:value={qhStart} aria-label="Início do silêncio" />
          <span>e</span>
          <input type="time" bind:value={qhEnd} aria-label="Fim do silêncio" />
          <button class="am-quiet-save" onclick={saveQuietHours}>Salvar</button>
        </div>
        {#if qhMsg}<div class="am-msg">{qhMsg}</div>{/if}
      </div>
    {/if}

    <div class="am-sep"></div>
    <button class="am-item" role="menuitem" onclick={reconnect}>
      <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M21 12a9 9 0 1 1-3-6.7L21 8"/><path d="M21 3v5h-5"/></svg>
      Reconectar
    </button>

    <div class="am-sep"></div>
    <button class="am-item am-danger" role="menuitem" onclick={logout}>
      <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4"/><path d="M16 17l5-5-5-5"/><path d="M21 12H9"/></svg>
      Sair
    </button>
{/snippet}

{#if embedded}
  <!-- Drawer do mobile: corpo inline, sem portal/backdrop/posição. O drawer é o "card". -->
  <div class="am-embedded">
    {@render menuBody()}
  </div>
{:else if open}
  <div use:portal>
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <div class="am-backdrop" role="button" tabindex="-1" aria-label="Fechar menu da conta" onclick={onClose}></div>
  <div class="am-card" role="menu" style="left: {pos.left}px; bottom: {pos.bottom}px;">
    <div class="am-head">
      <span class="am-avatar" aria-hidden="true">{initials}</span>
      <span class="am-who">
        <span class="am-name">{accountName}</span>
        {#if accountSub}<span class="am-sub">{accountSub}</span>{/if}
      </span>
    </div>
    <div class="am-sep"></div>
    {@render menuBody()}
  </div>
  </div>
{/if}

<style>
  /* Backdrop full-screen: captura o clique-fora pra fechar. */
  .am-backdrop { position: fixed; inset: 0; z-index: 60; }

  /* Card: FIXED, ancorado ao rodapé via JS (getBoundingClientRect). Fixed escapa o overflow:hidden da
     sidebar/lista. left/bottom vêm do inline style. Rola por dentro se estourar a altura. */
  .am-card {
    position: fixed;
    z-index: 61;
    width: max-content;
    min-width: 260px;
    max-width: min(320px, calc(100vw - var(--space-6)));
    max-height: min(70vh, 560px);
    overflow-y: auto;
    background: var(--bg-elevated);
    border: 1px solid var(--border-default);
    border-radius: var(--radius-lg);
    box-shadow: 0 16px 48px rgba(0, 0, 0, 0.5);
    padding: var(--space-1) 0;
    animation: am-in 160ms var(--ease-out) both;
  }
  @keyframes am-in {
    from { opacity: 0; transform: translateY(6px) scale(0.98); }
    to   { opacity: 1; transform: translateY(0) scale(1); }
  }

  /* Embedded (drawer mobile): sem card/portal — os itens (.am-*) já se estilizam sozinhos; só o
     respiro do rodapé pra safe-area. */
  .am-embedded { padding-bottom: calc(env(safe-area-inset-bottom) + var(--space-2)); }

  .am-head { display: flex; align-items: center; gap: var(--space-3); padding: var(--space-3) var(--space-4) var(--space-2); }
  .am-avatar {
    width: 34px; height: 34px; flex-shrink: 0; border-radius: 50%;
    display: grid; place-items: center;
    background: linear-gradient(135deg, var(--accent), #a06de0);
    color: #fff; font-size: var(--text-xs); font-weight: 700;
  }
  .am-who { min-width: 0; display: flex; flex-direction: column; }
  .am-name { font-size: var(--text-sm); font-weight: 600; color: var(--text-primary); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .am-sub { font-size: var(--text-xs); color: var(--text-muted); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }

  .am-sep { height: 1px; background: var(--border-subtle); margin: var(--space-1) 0; }

  .am-section {
    font-size: var(--text-xs); font-weight: 600; letter-spacing: 0.04em; text-transform: uppercase;
    color: var(--text-muted); padding: var(--space-2) var(--space-4) var(--space-1);
  }

  /* Linha de servidor: dot + label (+ tag "ativo" no desktop) + renomear + remover. */
  .am-srv { display: flex; align-items: center; gap: var(--space-2); padding: 0 var(--space-2) 0 var(--space-4); min-height: 40px; }
  .am-srv.on { background: var(--accent-dim); }
  .am-dot { width: 8px; height: 8px; border-radius: 50%; flex-shrink: 0; }
  .am-srv-pick {
    flex: 1; min-width: 0; display: flex; align-items: center; gap: var(--space-2); height: 40px; min-height: 0;
    padding: 0; text-align: left; justify-content: flex-start; color: var(--text-primary); font-size: var(--text-sm);
  }
  .am-srv-label { flex: 1; min-width: 0; font-size: var(--text-sm); color: var(--text-primary); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .am-tag { flex-shrink: 0; font-size: 10px; font-weight: 600; color: var(--accent); }
  .am-srv-rename { width: 30px; height: 32px; min-height: 0; flex-shrink: 0; color: var(--text-muted); font-size: var(--text-sm); border-radius: var(--radius-sm); }
  .am-srv-rename:hover { color: var(--accent); background: var(--bg-hover); }
  .am-srv-del { width: 30px; height: 32px; min-height: 0; flex-shrink: 0; color: var(--text-muted); font-size: var(--text-lg); line-height: 1; border-radius: var(--radius-sm); }
  .am-srv-del:hover { color: var(--error); background: var(--bg-hover); }
  .am-srv-edit {
    flex: 1; min-width: 0; height: 32px; padding: 0 var(--space-2);
    background: var(--bg-base); border: 1px solid var(--accent); border-radius: var(--radius-sm);
    color: var(--text-primary); font-family: var(--font-ui); font-size: 16px; outline: none;
  }

  /* Item de menu (ícone + rótulo). */
  .am-item {
    display: flex; align-items: center; gap: var(--space-3);
    width: 100%; min-height: 44px; padding: var(--space-2) var(--space-4);
    text-align: left; justify-content: flex-start;
    color: var(--text-primary); font-size: var(--text-sm); border-radius: 0;
    transition: background 150ms var(--ease-out), color 150ms var(--ease-out);
  }
  .am-item svg { flex-shrink: 0; color: var(--text-secondary); }
  .am-item:hover { background: var(--bg-hover); }
  .am-item:active { background: var(--bg-hover); }
  .am-item:disabled { color: var(--text-muted); }
  .am-danger { color: var(--error); }
  .am-danger svg { color: var(--error); }
  .am-danger:hover { background: rgba(255, 69, 58, 0.1); }

  .am-msg { font-size: var(--text-xs); color: var(--text-muted); padding: 2px var(--space-4) var(--space-1); }

  /* Horas silenciosas: cabeçalho (ícone + rótulo) + par de <input type="time"> + Salvar. */
  .am-quiet { padding: var(--space-1) var(--space-4) var(--space-2); }
  .am-quiet-head { display: flex; align-items: center; gap: var(--space-2); font-size: var(--text-sm); color: var(--text-secondary); padding: var(--space-1) 0; }
  .am-quiet-head svg { flex-shrink: 0; color: var(--text-secondary); }
  .am-quiet-row { display: flex; align-items: center; gap: var(--space-2); font-size: var(--text-sm); color: var(--text-secondary); }
  .am-quiet-row input[type='time'] {
    min-width: 0; flex: 1;
    background: var(--bg-base); border: 1px solid var(--border-default); border-radius: var(--radius-sm);
    color: var(--text-primary); font-size: var(--text-sm); padding: 4px 6px;
  }
  .am-quiet-save {
    flex-shrink: 0; min-height: 0; font-size: var(--text-xs); font-weight: 600; color: var(--accent);
    padding: 5px 10px; border-radius: var(--radius-full); border: 1px solid var(--accent);
  }
  .am-quiet-save:hover { background: var(--accent); color: #fff; }

  button:focus-visible { outline: 2px solid var(--accent); outline-offset: -2px; }

  @media (prefers-reduced-motion: reduce) {
    .am-card { animation: none; }
  }
</style>
