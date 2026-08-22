<script lang="ts">
  import { serverColor } from '../lib/auth';
  import ServerEditSheet from './ServerEditSheet.svelte';
  import { vaultPush } from '../lib/vaultPush.svelte';
  import type { Server } from '../lib/auth';
  import * as m from '../paraglide/messages';

  // Linhas de servidor + botão de editar (abre ServerEditSheet) + Adicionar. Extraído do AccountMenu
  // (Task 4a) pra ser reusado na tela Servidores das Configurações (Task 4b): mesmo markup nos dois.
  interface Props {
    servers: Server[];
    activeId?: string | null;
    // Alvo das telas de config de servidor. Quando o pai passa `onPickTarget`, a LINHA INTEIRA é o
    // seletor de alvo e o conceito de "ativo" nem aparece — ele não é uma escolha do usuário: quem
    // troca o ativo é a rota (`App.applyRouteServer`), a cada sessão aberta. Um botão "ativo" ali
    // era um controle que mente: apertar não muda nada que se veja, e o próximo clique numa sessão
    // desfaz. O que se escolhe nesta tela é só ONDE as configs de servidor vão ser gravadas.
    targetId?: string | null;
    onPickTarget?: (id: string) => void;
    // Trocar o ativo à mão: só o menu de conta (que não escolhe alvo de config).
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

  // Quem está marcado na lista: o ALVO das configs quando o pai escolhe alvo, senão o ativo.
  const marcado = (id: string) => (onPickTarget ? id === targetId : id === activeId);

  // Edicao (nome + endereco + token) numa FOLHA, nao mais em dois editores inline dentro da linha:
  // a linha tem 40px e tres botoes de 30px, e no celular nao dava pra ver o que estava gravado nem
  // trocar so o token (o campo antigo nascia vazio, sem nada com que comparar).
  let editando = $state<Server | null>(null);
  // Le da lista viva, nao do objeto capturado no clique: com a folha aberta, um rename vindo do sync
  // (outra aba/aparelho) deixaria os campos mostrando o valor velho.
  const emEdicao = $derived(editando ? servers.find((s) => s.id === editando!.id) ?? null : null);

</script>

<div class="sm-section">{m.config_modal_servidores()}</div>
{#each servers as s (s.id)}
  <div class="sm-srv" class:on={marcado(s.id)}>
    {#if onPickTarget || onSwitchActive}
      <button class="sm-srv-pick"
        onclick={() => (onPickTarget ? onPickTarget(s.id) : onSwitchActive!(s.id))}
        aria-pressed={onPickTarget ? s.id === targetId : undefined}
        aria-label={onPickTarget ? m.servidor_gravar_aria({ nome: s.label }) : undefined}>
        <span class="sm-dot" style="background: {serverColor(s.id)};" aria-hidden="true"></span>
        <span class="sm-srv-label">{s.label}</span>
        {#if marcado(s.id)}<span class="sm-tag">{onPickTarget ? m.servidor_escolhido() : m.servidor_ativo()}</span>{/if}
      </button>
    {:else}
      <span class="sm-dot" style="background: {serverColor(s.id)};" aria-hidden="true"></span>
      <span class="sm-srv-label">{s.label}</span>
    {/if}
    <button class="sm-srv-edit-btn" onclick={() => (editando = s)}
            aria-label={m.servidor_editar_aria({ nome: s.label })} title={m.servidor_editar_titulo()}>✎</button>
    {#if servers.length > 1 || podeRemoverUltimo}
      <button class="sm-srv-del" onclick={() => onRemove(s.id)} aria-label={m.servidor_remover_aria({ nome: s.label })}>×</button>
    {/if}
  </div>
{/each}

<!-- Só aparece quando o push do vault FALHOU (ou o sync está deslogado). 'idle' = ninguém
     configurou sync, e nesse caso não há o que avisar. Sucesso também não vira linha: a
     mudança já está visível na lista, confirmar cada uma seria ruído. -->
{#if vaultPush.estado === 'error' || vaultPush.estado === 'locked'}
  <div class="sm-sync-warn" role="status">⚠ {vaultPush.detalhe}</div>
{/if}

<button class="sm-item" type="button" role={menuitem ? 'menuitem' : undefined} onclick={onAdd}>
  <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" aria-hidden="true"><path d="M12 5v14M5 12h14"/></svg>
  {m.sessao_adicionar_servidor()}
</button>

<ServerEditSheet
  open={!!emEdicao}
  server={emEdicao}
  onClose={() => (editando = null)}
  {onRename}
  {onUpdateToken}
/>

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

  /* Linha de servidor: dot + label (+ tag "ativo" no desktop) + editar + remover. */
  .sm-srv { display: flex; align-items: center; gap: var(--space-2); padding: 0 var(--space-2) 0 var(--space-4); min-height: 44px; }
  .sm-srv.on { background: var(--accent-dim); }
  .sm-dot { width: 8px; height: 8px; border-radius: 50%; flex-shrink: 0; }
  .sm-srv-pick {
    flex: 1; min-width: 0; display: flex; align-items: center; gap: var(--space-2); height: 40px; min-height: 0;
    padding: 0; text-align: left; justify-content: flex-start; color: var(--text-primary); font-size: var(--text-sm);
  }
  .sm-srv-label { flex: 1; min-width: 0; font-size: var(--text-sm); color: var(--text-primary); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .sm-tag { flex-shrink: 0; font-size: 10px; font-weight: 600; color: var(--accent); }
  .sm-srv-edit-btn { width: 40px; height: 40px; min-height: 0; flex-shrink: 0; color: var(--text-muted); font-size: var(--text-sm); border-radius: var(--radius-sm); }
  .sm-srv-edit-btn:hover { color: var(--accent); background: var(--bg-hover); }
  .sm-srv-del { width: 40px; height: 40px; min-height: 0; flex-shrink: 0; color: var(--text-muted); font-size: var(--text-lg); line-height: 1; border-radius: var(--radius-sm); }
  .sm-srv-del:hover { color: var(--error); background: var(--bg-hover); }
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
