<script lang="ts">
  import { serverColor, validarPareamento } from '../lib/auth';
  import { vaultPush } from '../lib/vaultPush.svelte';
  import type { Server } from '../lib/auth';

  // Linhas de servidor + edição inline (renomear / trocar token) + Adicionar. Extraído do AccountMenu
  // (Task 4a) pra ser reusado na tela Servidores das Configurações (Task 4b): mesmo markup, mesmo
  // parse de URL de pareamento, um arquivo só.
  interface Props {
    servers: Server[];
    activeId?: string | null;
    // Linha "Editar" do alvo de config (desktop): quem está sendo editado fica marcado.
    targetId?: string | null;
    onPickTarget?: (id: string) => void;
    // Botão de selecionar ativo só existe quando o pai sabe trocar (desktop). Drawer mobile não troca.
    onSwitchActive?: (id: string) => void;
    // Remover o ÚLTIMO servidor: no AccountMenu desktop o × some quando sobra 1 (remover o ativo
    // derruba a sessão sem aviso); na tela Servidores o último TEM que ser removível — remover tudo
    // dispara o logout global, única saída pra deslogar. Desvio aprovado do plano 4b, prop optativa.
    podeRemoverUltimo?: boolean;
    // Semântica de MENU só quando há ancestral role="menu" (popover do AccountMenu desktop). A tela
    // Servidores das Configurações renderiza isto dentro de role="dialog" — menuitem ali seria
    // papel inválido (WCAG 4.1.2). Default seguro: botão comum. (round 7)
    menuitem?: boolean;
    onRename: (id: string, label: string) => void;
    onUpdateToken: (id: string, token: string) => boolean;
    onRemove: (id: string) => void;
    onAdd: () => void;
  }
  let {
    servers, activeId = null, targetId = null, onPickTarget, onSwitchActive,
    podeRemoverUltimo = false, menuitem = false,
    onRename, onUpdateToken, onRemove, onAdd,
  }: Props = $props();

  // Rename inline de servidor: id em edição + valor do input. O pai persiste (renameServer + reagrega).
  let editingId = $state<string | null>(null);
  let editLabel = $state('');
  function startRename(id: string, current: string) {
    editingId = id;
    editLabel = current;
  }
  function saveRename() {
    if (editingId) onRename(editingId, editLabel);
    editingId = null;
  }

  // Edicao de TOKEN, separada do rename: mesmo padrao inline, mas o campo comeca VAZIO (nunca
  // pre-preenche com o token atual — segredo nao vai pra tela) e aceita tanto o token cru quanto a
  // URL de pareamento inteira, pra quem cola do QR nao ter que extrair nada na mao.
  let editingTokenId = $state<string | null>(null);
  let editToken = $state('');
  let tokenError = $state('');
  let tokenInputEl = $state<HTMLInputElement | null>(null);
  function startEditToken(id: string) {
    editingTokenId = id;
    editToken = '';
    tokenError = '';
    editingId = null;      // os dois modos usam a mesma linha; abrir um fecha o outro
    // NAO limpa o vaultPush aqui: abrir o campo nao conserta push nenhum. Limpando no clique, um
    // erro NAO RESOLVIDO sumia da tela so por o usuario abrir o editor de outro servidor e apertar
    // Esc — e "o aviso sumiu" se le como "resolvido", enquanto os outros aparelhos seguem com o
    // token velho. Some so quando ha uma tentativa NOVA (saveToken).
  }
  function saveToken() {
    const id = editingTokenId;
    const text = editToken.trim();
    // Vazio = desistiu. Gravar string vazia desautenticaria o servidor calado, e o campo em branco
    // e exatamente o que sobra quando o usuario abre e clica fora.
    if (!id || !text) {
      editingTokenId = null;
      editToken = '';
      tokenError = '';
      return;
    }

    const parsed = validarPareamento(text, { aceitarTokenCru: true });
    // RECUSA em vez de aceitar lixo como token: URL malformada (esquema torto, sem ?token=, token
    // duplicado/espacado) ou token cru com espaço. O campo e password — o usuario nao ve o lixo que
    // ficaria salvo; gravar aqui era o 401 sem pista que chegava depois. O campo CONTINUA aberto,
    // com o texto, pra corrigir.
    if (!parsed) {
      tokenError = text.includes('://')
        ? 'URL de pareamento inválida — cole a URL completa (com ?token=) ou só o token.'
        : 'token inválido — não pode conter espaços.';
      tokenInputEl?.focus();   // erro associado ao campo (aria-describedby): foco onde corrigir
      return;
    }
    // So o TOKEN. O botao diz "Trocar token": colar a URL de pareamento de outra maquina nao pode
    // reapontar calado um servidor ja cadastrado (com label e historico) pra outro host.
    const token = parsed.token;
    const alvo = servers.find((s) => s.id === id);
    const outroHost = parsed.base && alvo && parsed.base.replace(/\/+$/, '') !== alvo.baseUrl.replace(/\/+$/, '');

    vaultPush.clear();                          // tentativa NOVA: agora sim zera o resultado antigo
    const ok = onUpdateToken(id, token);
    if (!ok) {
      // false = o id sumiu (removido noutra aba/aparelho pelo sync entre abrir e salvar). Raro,
      // mas indistinguivel de sucesso se ficasse calado — o campo ja teria fechado.
      tokenError = 'esse servidor não existe mais nesta lista.';
      return;
    }
    editingTokenId = null;
    editToken = '';
    tokenError = outroHost
      ? `token trocado. O endereço NÃO mudou (segue ${alvo!.baseUrl}) — pra trocar o host, remova e adicione de novo.`
      : '';
  }
</script>

<div class="sm-section">Servidores</div>
{#each servers as s (s.id)}
  <div class="sm-srv" class:on={s.id === activeId}>
    {#if editingTokenId === s.id}
      <span class="sm-dot" style="background: {serverColor(s.id)};" aria-hidden="true"></span>
      <!-- svelte-ignore a11y_autofocus -->
      <input
        class="sm-srv-edit"
        type="password"
        autocomplete="off"
        bind:value={editToken}
        bind:this={tokenInputEl}
        placeholder="token novo ou URL de pareamento"
        onclick={(e) => e.stopPropagation()}
        onkeydown={(e) => { if (e.key === 'Enter') saveToken(); if (e.key === 'Escape') { editingTokenId = null; editToken = ''; } }}
        onblur={saveToken}
        autofocus
        aria-label={`Novo token de ${s.label}`}
        aria-invalid={tokenError ? true : undefined}
        aria-describedby={tokenError ? 'sm-token-err' : undefined}
      />
    {:else if editingId === s.id}
      <span class="sm-dot" style="background: {serverColor(s.id)};" aria-hidden="true"></span>
      <!-- svelte-ignore a11y_autofocus -->
      <input
        class="sm-srv-edit"
        bind:value={editLabel}
        onclick={(e) => e.stopPropagation()}
        onkeydown={(e) => { if (e.key === 'Enter') saveRename(); if (e.key === 'Escape') editingId = null; }}
        onblur={saveRename}
        autofocus
        aria-label="Novo nome do servidor"
      />
    {:else if onSwitchActive}
      <button class="sm-srv-pick" onclick={() => onSwitchActive(s.id)}>
        <span class="sm-dot" style="background: {serverColor(s.id)};" aria-hidden="true"></span>
        <span class="sm-srv-label">{s.label}</span>
        {#if s.id === activeId}<span class="sm-tag">ativo</span>{/if}
      </button>
      <button class="sm-srv-rename" onclick={() => startRename(s.id, s.label)} aria-label={`Renomear ${s.label}`} title="Renomear">✎</button>
      <button class="sm-srv-rename" onclick={() => startEditToken(s.id)} aria-label={`Trocar token de ${s.label}`} title="Trocar token">🔑</button>
      {#if servers.length > 1 || podeRemoverUltimo}
        <button class="sm-srv-del" onclick={() => onRemove(s.id)} aria-label={`Remover ${s.label}`}>×</button>
      {/if}
    {:else}
      <span class="sm-dot" style="background: {serverColor(s.id)};" aria-hidden="true"></span>
      <span class="sm-srv-label">{s.label}</span>
      <button class="sm-srv-rename" onclick={() => startRename(s.id, s.label)} aria-label={`Renomear ${s.label}`} title="Renomear">✎</button>
      <button class="sm-srv-rename" onclick={() => startEditToken(s.id)} aria-label={`Trocar token de ${s.label}`} title="Trocar token">🔑</button>
      {#if servers.length > 1 || podeRemoverUltimo}
        <button class="sm-srv-del" onclick={() => onRemove(s.id)} aria-label={`Remover ${s.label}`}>×</button>
      {/if}
    {/if}
    {#if onPickTarget}
      <button class="sm-target" class:on={s.id === targetId}
        onclick={() => onPickTarget(s.id)}
        aria-label={`Editar configurações de ${s.label}`}>
        {s.id === targetId ? '● editando' : 'Editar'}
      </button>
    {/if}
  </div>
{/each}
<!-- Só aparece quando o push do vault FALHOU (ou o sync está deslogado). 'idle' = ninguém
     configurou sync, e nesse caso não há o que avisar. Sucesso também não vira linha: a
     mudança já está visível na lista, confirmar cada uma seria ruído. -->
<!-- Resultado da própria edição (recusa de URL inválida, servidor sumido, host preservado):
     separado do aviso de sync, que é sobre o push pro hub e não sobre o que você digitou. -->
{#if tokenError}
  <div id="sm-token-err" class="sm-sync-warn" role="alert">{tokenError}</div>
{/if}

{#if vaultPush.estado === 'error' || vaultPush.estado === 'locked'}
  <div class="sm-sync-warn" role="status">⚠ {vaultPush.detalhe}</div>
{/if}

<button class="sm-item" type="button" role={menuitem ? 'menuitem' : undefined} onclick={onAdd}>
  <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" aria-hidden="true"><path d="M12 5v14M5 12h14"/></svg>
  Adicionar servidor
</button>

<style>
  .sm-section {
    font-size: var(--text-xs); font-weight: 600; letter-spacing: 0.04em; text-transform: uppercase;
    color: var(--text-muted); padding: var(--space-2) var(--space-4) var(--space-1);
  }

  /* Aviso de push do vault que nao subiu. Fica logo abaixo dos servidores porque e ali que a
     acao aconteceu — mensagem longe do lugar do erro ninguem associa. */
  .sm-sync-warn {
    font-size: var(--text-xs); color: var(--warning); line-height: 1.4;
    padding: var(--space-1) var(--space-4) var(--space-2);
  }

  /* Linha de servidor: dot + label (+ tag "ativo" no desktop) + renomear + remover. */
  .sm-srv { display: flex; align-items: center; gap: var(--space-2); padding: 0 var(--space-2) 0 var(--space-4); min-height: 40px; }
  .sm-srv.on { background: var(--accent-dim); }
  .sm-dot { width: 8px; height: 8px; border-radius: 50%; flex-shrink: 0; }
  .sm-srv-pick {
    flex: 1; min-width: 0; display: flex; align-items: center; gap: var(--space-2); height: 40px; min-height: 0;
    padding: 0; text-align: left; justify-content: flex-start; color: var(--text-primary); font-size: var(--text-sm);
  }
  .sm-srv-label { flex: 1; min-width: 0; font-size: var(--text-sm); color: var(--text-primary); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .sm-tag { flex-shrink: 0; font-size: 10px; font-weight: 600; color: var(--accent); }
  .sm-srv-rename { width: 30px; height: 32px; min-height: 0; flex-shrink: 0; color: var(--text-muted); font-size: var(--text-sm); border-radius: var(--radius-sm); }
  .sm-srv-rename:hover { color: var(--accent); background: var(--bg-hover); }
  .sm-srv-del { width: 30px; height: 32px; min-height: 0; flex-shrink: 0; color: var(--text-muted); font-size: var(--text-lg); line-height: 1; border-radius: var(--radius-sm); }
  .sm-srv-del:hover { color: var(--error); background: var(--bg-hover); }
  .sm-srv-edit {
    flex: 1; min-width: 0; height: 32px; padding: 0 var(--space-2);
    background: var(--surface-inset); border: 1px solid var(--accent); border-radius: var(--radius-sm);
    color: var(--text-primary); font-family: var(--font-ui); font-size: 16px; outline: none;
  }
  .sm-srv-edit:focus-visible { outline: 2px solid var(--accent); outline-offset: -2px; }

  /* Botão "Editar" do alvo de config (SettingsModal): marca quem está sendo editado. */
  .sm-target {
    flex-shrink: 0; font-size: var(--text-xs); font-weight: 600; color: var(--text-secondary);
    border-radius: var(--radius-sm); padding: 4px 8px;
  }
  .sm-target.on { color: var(--accent); background: var(--accent-dim); }
  .sm-target:hover { color: var(--accent); background: var(--bg-hover); }

  /* Item de menu (ícone + rótulo). */
  .sm-item {
    display: flex; align-items: center; gap: var(--space-3);
    width: 100%; min-height: 44px; padding: var(--space-2) var(--space-4);
    text-align: left; justify-content: flex-start;
    color: var(--text-primary); font-size: var(--text-sm); border-radius: 0;
    transition: background 150ms var(--ease-out), color 150ms var(--ease-out);
  }
  .sm-item svg { flex-shrink: 0; color: var(--text-secondary); }
  .sm-item:hover { background: var(--bg-hover); }
  .sm-item:active { background: var(--bg-hover); }
  .sm-item:disabled { color: var(--text-muted); }

  button:focus-visible { outline: 2px solid var(--accent); outline-offset: -2px; }
</style>
