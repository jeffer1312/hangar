<script lang="ts">
  // Pílula de permissão em sessão viva — irmã de ClaudeEffortPopover.
  // Mostra os modos alcançáveis via BTab (4 ou 5, lidos ao vivo) e desabilita
  // os que só existem na criação (dontAsk isolado, bypassPermissions fora do ciclo).
  import * as m from '../paraglide/messages';
  import Popover from './Popover.svelte';
  import {
    rotuloPermissao, descricaoPermissao, permissaoSemFreio, MODOS_PERMISSAO,
  } from '../lib/permissaoRotulo';
  import IconPermissao from './icons/IconPermissao.svelte';

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
  {:else if sondavel === false && current === 'dontAsk'}
    <div class="vazio" style="padding: 10px;">
      <p style="margin:0 0 6px; font-size: var(--text-sm);"><strong>{current ?? m.composer_permissao()}</strong> — {m.permissao_dontask_sem_volta()}</p>
      <p class="dica" style="border:none; padding:0;">{m.permissao_dica()}</p>
    </div>
  {:else if modes.length === 0}
    <p class="dica">{m.permissao_dica()}</p>
  {:else}
    <ul class="lista">
      {#each MODOS_PERMISSAO as modo (modo)}
        {@const habilitado = habilitados.has(modo)}
        {@const ativo = current === modo}
        <li>
          <button
            class="linha"
            class:ativa={ativo}
            class:desabilitado={!habilitado}
            class:sem-freio={permissaoSemFreio(modo)}
            aria-pressed={ativo}
            aria-disabled={!habilitado}
            aria-describedby={!habilitado ? `perm-motivo-${modo}` : undefined}
            disabled={!!aplicando || !habilitado}
            data-foco={ativo ? true : undefined}
            onclick={() => escolher(modo)}
            title={modo}
          >
            <span class="glifo"><IconPermissao {modo} /></span>
            <span class="nome">
              <span class="rotulo">{rotuloPermissao(modo)}</span>
              <!-- A frase do modo indisponível fica em opacidade NORMAL: é ela que explica por que
                   a linha está apagada, e apagar as duas junto some com a explicação. -->
              <span class="desc" id={!habilitado ? `perm-motivo-${modo}` : undefined}>
                {descricaoPermissao(modo)}{#if !habilitado} · {m.permissao_so_criacao()}{/if}
              </span>
            </span>
            {#if aplicando === modo}
              <span class="tick" aria-hidden="true">…</span>
            {:else if ativo}
              <svg class="tick" width="16" height="16" viewBox="0 0 24 24" fill="none"
                stroke="currentColor" stroke-width="2.5" stroke-linecap="round"
                stroke-linejoin="round" aria-hidden="true">
                <polyline points="20 6 9 17 4 12" />
              </svg>
            {/if}
          </button>
        </li>
      {/each}
    </ul>
  {/if}
</Popover>

<style>
  .err { color: var(--error); font-size: var(--text-xs); margin: 8px 10px 0; }
  .vazio { color: var(--text-muted); font-size: var(--text-sm); text-align: center; padding: 14px 0; }
  .lista { list-style: none; margin: 0; padding: 4px 0; overflow-y: auto; }

  .linha {
    display: flex; align-items: flex-start; gap: 8px; width: 100%;
    padding: 8px 10px; background: transparent; border: none;
    color: var(--text-primary); font-size: var(--text-sm); text-align: left; cursor: pointer;
  }
  .linha:hover:not(:disabled):not(.desabilitado) { background: var(--bg-hover); }
  .linha.ativa:hover:not(:disabled) { background: var(--accent-dim); }
  .linha:disabled { cursor: default; }
  .linha.ativa { background: var(--accent-dim); color: var(--text-primary); }
  .linha.desabilitado { cursor: not-allowed; }

  .nome { flex: 1; min-width: 0; text-transform: none; display: flex; flex-direction: column; gap: 1px; }
  .rotulo { line-height: 1.25; }
  .desc { font-size: var(--text-xs); color: var(--text-muted); line-height: 1.3; }
  .glifo { flex: none; width: 1.6em; font-size: 0.85em; color: var(--text-muted); padding-top: 2px; }
  /* Os dois sem confirmação: tinta mais quente no glifo. Fundo de alerta fica pra erro — escolher
     um modo mais solto de propósito não é erro. */
  .linha.sem-freio .glifo { color: var(--warning); }
  .tick { flex: none; color: var(--accent); padding-top: 2px; }

  /* A linha apaga, a explicação não: é ela que diz por que a opção está fora de alcance. */
  .linha.desabilitado .rotulo,
  .linha.desabilitado .glifo { opacity: 0.5; }
</style>
