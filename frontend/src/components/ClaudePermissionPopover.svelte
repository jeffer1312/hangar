<script lang="ts">
  // Pílula de permissão em sessão viva — irmã de ClaudeEffortPopover.
  // Mostra os modos alcançáveis via BTab (4 ou 5, lidos ao vivo) e desabilita
  // os que só existem na criação (dontAsk isolado, bypassPermissions fora do ciclo).
  import * as m from '../paraglide/messages';
  import Popover from './Popover.svelte';

  const MODOS_TODOS = ['plan', 'auto', 'manual', 'acceptEdits', 'bypassPermissions', 'dontAsk'] as const;
  type Modo = typeof MODOS_TODOS[number];

  // Rótulo humano para a pill (curto). O id do CLI já é o nome real da flag —
  // dado, não interface — então o rótulo é o próprio id.
  function rotulo(modo: string): string {
    return modo;
  }

  interface Props {
    open: boolean;
    anchor: HTMLElement | null;
    current: string | null;
    modes: string[]; // ciclo vivo (4 ou 5) devolvido pelo GET
    sondavel?: boolean;
    carregando?: boolean;
    onApply: (modo: string) => Promise<void>;
    onClose: () => void;
  }
  let { open, anchor, current, modes, sondavel = true, carregando = false, onApply, onClose }: Props = $props();

  let err = $state<string | null>(null);
  let aplicando = $state<string | null>(null);

  const habilitados = $derived(new Set(modes));

  $effect(() => {
    if (open) { err = null; aplicando = null; }
  });

  async function escolher(modo: string) {
    if (aplicando) return;
    // desabilitado não deveria chegar aqui, mas guarda
    if (!habilitados.has(modo)) return;
    if (modo === current) { onClose(); return; }
    aplicando = modo;
    err = null;
    try {
      await onApply(modo);
    } catch (e) {
      err = e instanceof Error ? e.message : m.comum_falha_aplicar();
      aplicando = null;
      return;
    }
    aplicando = null;
    onClose();
  }
</script>

<Popover {open} {anchor} {onClose} width={240} ariaLabel={m.composer_permissao()}>
  {#if err}
    <p class="err" role="alert">{err}</p>
  {/if}

  {#if carregando}
    <p class="vazio">{m.comum_carregando()}</p>
  {:else if !sondavel}
    <div class="vazio" style="padding: 10px;">
      <p style="margin:0 0 6px; font-size: var(--text-sm);"><strong>{current ?? m.composer_permissao()}</strong> — {m.permissao_dontask_sem_volta()}</p>
      <p class="dica" style="border:none; padding:0;">{m.permissao_dica()}</p>
    </div>
  {:else}
    <ul class="lista">
      {#each MODOS_TODOS as modo (modo)}
        {@const habilitado = habilitados.has(modo)}
        {@const ativo = current === modo}
        <li>
          <button
            class="linha"
            class:ativa={ativo}
            class:desabilitado={!habilitado}
            aria-pressed={ativo}
            aria-disabled={!habilitado}
            disabled={!!aplicando || !habilitado}
            data-foco={ativo ? true : undefined}
            onclick={() => escolher(modo)}
            title={!habilitado ? m.permissao_so_criacao() : undefined}
          >
            <span class="nome">{rotulo(modo)}</span>
            {#if !habilitado}
              <span class="dica-desab" aria-hidden="true">{m.permissao_so_criacao_curto()}</span>
            {:else if aplicando === modo}
              <span class="tick" aria-hidden="true">…</span>
            {:else if ativo}
              <svg class="tick" width="16" height="16" viewBox="0 0 24 24" fill="none"
                stroke="currentColor" stroke-width="2.5" stroke-linecap="round"
                stroke-linejoin="round" aria-hidden="true">
                <polyline points="20 6 9 17 4 12" />
              </svg>
            {/if}
          </button>
          {#if !habilitado}
            <span class="motivo">{m.permissao_so_criacao()}</span>
          {/if}
        </li>
      {/each}
    </ul>
    <p class="dica">{m.permissao_dica()}</p>
  {/if}
</Popover>

<style>
  .err { color: var(--error); font-size: var(--text-xs); margin: 8px 10px 0; }
  .vazio { color: var(--text-muted); font-size: var(--text-sm); text-align: center; padding: 14px 0; }
  .lista { list-style: none; margin: 0; padding: 4px 0; overflow-y: auto; }

  .linha {
    display: flex; align-items: center; gap: 6px; width: 100%;
    padding: 6px 10px; background: transparent; border: none;
    color: var(--text-primary); font-size: var(--text-sm); text-align: left; cursor: pointer;
  }
  .linha:hover:not(:disabled):not(.desabilitado) { background: var(--bg-hover); }
  .linha.ativa:hover:not(:disabled) { background: var(--accent-dim); }
  .linha:disabled { cursor: default; }
  .linha.ativa { background: var(--accent-dim); color: var(--text-primary); }
  .linha.desabilitado { opacity: 0.5; cursor: not-allowed; }

  .nome { flex: 1; text-transform: none; }
  .dica-desab { font-size: var(--text-xs); color: var(--text-muted); flex: none; }
  .tick { flex: none; color: var(--accent); }

  .motivo {
    display: block; font-size: var(--text-xs); color: var(--text-muted);
    padding: 0 10px 4px 10px;
  }

  .dica {
    font-size: var(--text-xs); color: var(--text-muted);
    padding: 6px 10px 8px; margin: 0; border-top: 1px solid var(--border-subtle);
  }
</style>
