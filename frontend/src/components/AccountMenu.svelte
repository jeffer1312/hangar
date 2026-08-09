<script lang="ts">
  import { pushSupported } from '../lib/push';
  import { getActiveId } from '../lib/auth';
  import { abrirConfig } from '../lib/configNav';
  import ServerManager from './ServerManager.svelte';
  import PushQuiet from './PushQuiet.svelte';
  import type { Server } from '../lib/auth';

  // Menu de CONTA (estilo Claude): tudo que é conta/config vive aqui, fora da navegação. Popover que
  // abre pra CIMA a partir do avatar no rodapé — mesma peça no mobile (SessionList) e no desktop
  // (Sidebar), diferindo só pelos handlers que o pai passa (trocar servidor só existe no desktop).
  // As linhas de servidor e o bloco de push vivem nos componentes extraídos (ServerManager/PushQuiet,
  // Task 4a) — o mesmo fluxo é reusado pela tela Servidores das Configurações (Task 4b). Ações que
  // dependem de UI do pai (adicionar/remover servidor, reconectar, sair) vêm por callback.
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
    // Recebe o TOKEN ja extraido e validado (o parse mora num arquivo so — nos dois pais
    // viraria validacao duplicada, e Sidebar/SessionList sao justamente os que vivem divergindo).
    // Devolve false quando o servidor nao existe mais, pra UI poder dizer isso em vez de fingir.
    onUpdateServerToken: (id: string, token: string) => boolean;
    onRemoveServer: (id: string) => void;
    onAddServer: () => void;
    onReconnect: () => void;
    onLogout: () => void;
  }
  let {
    open, onClose, initials, accountName, accountSub = null, servers, anchorEl = null, embedded = false, activeId = null,
    onSwitchServer, onRenameServer, onUpdateServerToken, onRemoveServer, onAddServer, onReconnect, onLogout,
  }: Props = $props();

  // Portal pro <body>: a sidebar tem `backdrop-filter` (liquid glass no Chromium), que vira bloco de
  // contenção e RECORTA até `position: fixed` — o menu (e o backdrop) ficavam presos dentro dela. Mover
  // os nós pro body escapa qualquer ancestral filtrado/transformado.
  function portal(node: HTMLElement) {
    document.body.appendChild(node);
    return { destroy() { node.remove(); } };
  }

  // Servidor-alvo das ações de config. No mobile a lista é AGREGADA (sem "ativo"), então sem isto
  // "Horas silenciosas"/"Configurações" batiam no servidor globalmente ativo — podia ser OUTRA
  // máquina. Resolve do array VIVO: um snapshot congelado devolveria o token de quando o drawer
  // abriu, e trocar o token na mesma linha passaria a mandar o antigo. Sumiu de `servers` -> null,
  // e a UI desabilita em vez de cair no primeiro da lista.
  const activeServer = $derived(servers.find((s) => s.id === activeId) ?? null);

  // Posição do card: FIXED, medida da âncora (rodapé) via getBoundingClientRect. Abre pra cima.
  let pos = $state({ left: 0, bottom: 0 });
  $effect(() => {
    if (!open || !anchorEl) return;
    const r = anchorEl.getBoundingClientRect();
    pos = { left: r.left, bottom: window.innerHeight - r.top + 8 }; // 8px acima do avatar
  });

  // Ações que disparam UI/fluxo do pai fecham o menu antes (o pai abre seu sheet/confirm/reload).
  // O modal vive FORA do menu (o menu fecha ao abrir): renderizado no fim do componente, sobrevive.
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
    <ServerManager
      {servers}
      {activeId}
      onSwitchActive={onSwitchServer ? (id) => { onClose(); onSwitchServer(id); } : undefined}
      onRename={onRenameServer}
      onUpdateToken={onUpdateServerToken}
      onRemove={onRemoveServer}
      onAdd={addServer}
    />
    {#if pushSupported()}
      <div class="am-sep"></div>
      <!-- PushQuiet com o alvo certo por view: desktop popover e GLOBAL (o enablePush assina em
           todos); drawer mobile mira o servidor resolvido, ou 'unavailable' quando sumiu — nunca
           cai nas funções globais, que leriam a janela de outra máquina como se fosse desta. O
           `open` so decide o LOAD (o componente fica montado sempre — fechar/reabrir o menu nao
           pode perder busy/resultado/Janela carregada). -->
      <PushQuiet {open} target={embedded ? (activeServer ? { mode: 'server', server: activeServer } : { mode: 'unavailable' }) : { mode: 'global' }} />
    {/if}

    <div class="am-sep"></div>
    <!-- Porta única de configuração: Aparência (do aparelho) + config do servidor + Motores, cada
         uma numa sub-tela do mesmo modal. Fica aqui junto do resto de conta/servidor, que é onde o
         usuário já procura ajuste. -->
    <!-- SEM `disabled`: o item antigo desabilitava quando o drawer ficava sem servidor alvo, mas
         agora ele também abre a Aparência, que é preferência do APARELHO e não depende de servidor
         nenhum. Quem perde entrada são só as duas linhas DE SERVIDOR, dentro do modal, com o motivo
         escrito na própria linha (`semServidor`). -->
    <!-- `onClose?.()` NAO e opcional: no celular ele e `drawerOpen = false` (SessionList.svelte:803).
         Sem ele o drawer fica aberto ATRAS do painel e reaparece quando o painel fecha.
         Navegar direto do leaf e o precedente do proprio arquivo vizinho: SessionList.svelte:789 e :793
         fazem `window.location.hash = '#/archive'` e `'#/costs'` na mao, sem prop drilling. -->
    <button class="am-item" role="menuitem"
            onclick={() => {
              const alvo = embedded ? (activeServer?.id ?? null) : getActiveId();
              onClose?.();
              abrirConfig('root', alvo);
            }}>
      <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 1 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06A1.65 1.65 0 0 0 4.6 15a1.65 1.65 0 0 0-1.51-1H3a2 2 0 1 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.6a1.65 1.65 0 0 0 1-1.51V3a2 2 0 1 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9c.14.63.68 1.1 1.51 1H21a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"/></svg>
      Configurações
    </button>

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
  <!-- Drawer do mobile: corpo inline, sem portal/backdrop/posição. O drawer é o "card". Fica
       montado com o drawer fechado (estado preservado), mas `inert` tira a subtree do Tab e da
       árvore de acessibilidade — sem ele, o conteúdo escondido continuava focável via Tab. -->
  <div class="am-embedded" inert={embedded && !open}>
    {@render menuBody()}
  </div>
{:else}
  <!-- Popover SEMPRE montado, visibilidade via CSS (.open): os filhos (ServerManager/PushQuiet)
       guardam estado proprio (rascunho de rename/token, busy/resultado de push) e desmontar no
       fechar PERDERIA isso — o AccountMenu antigo guardava esses estados aqui mesmo, e eles
       sobreviviam fechar/reabrir. Fechado = display:none (inerte, fora do foco). -->
  <div use:portal>
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <div class="am-backdrop" class:open role="button" tabindex="-1" aria-label="Fechar menu da conta" onclick={onClose}></div>
  <div class="am-card" class:open role="menu" style="left: {pos.left}px; bottom: {pos.bottom}px;">
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
  /* Backdrop full-screen: captura o clique-fora pra fechar. display:none quando fechado (o nó vive
     no body pra sempre — o portal monta uma vez e a visibilidade é CSS). */
  .am-backdrop { position: fixed; inset: 0; z-index: 60; display: none; }
  .am-backdrop.open { display: block; }

  /* Card: FIXED, ancorado ao rodapé via JS (getBoundingClientRect). Fixed escapa o overflow:hidden da
     sidebar/lista. left/bottom vêm do inline style. Rola por dentro se estourar a altura. */
  .am-card {
    position: fixed;
    z-index: 61;
    display: none;
    width: max-content;
    min-width: 260px;
    max-width: min(320px, calc(100vw - var(--space-6)));
    max-height: min(70vh, 560px);
    overflow-y: auto;
    background: var(--surface-raised);
    border: 1px solid var(--border-default);
    border-radius: var(--radius-lg);
    box-shadow: 0 16px 48px rgba(0, 0, 0, 0.5);
    padding: var(--space-1) 0;
    animation: am-in 160ms var(--ease-out) both;
  }
  .am-card.open { display: block; }
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

  button:focus-visible { outline: 2px solid var(--accent); outline-offset: -2px; }

  @media (prefers-reduced-motion: reduce) {
    .am-card { animation: none; }
  }
</style>
