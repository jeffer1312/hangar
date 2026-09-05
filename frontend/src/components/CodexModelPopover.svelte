<script lang="ts">
  // Seletor de modelo do Codex ancorado na pill do composer — irmão do KimiModelPopover. Era a
  // última das quatro pills que ainda abria FOLHA; a escolha é a mesma, o que muda é o formato.
  //
  // Sem busca, ao contrário do Pi e do Kimi: o catálogo do Codex tem meia dúzia de linhas (seis
  // nesta máquina, em 30/08/2026), e um campo de busca sobre seis itens é enfeite que rouba a
  // primeira linha do teclado.
  //
  // O `current` vem do próprio GET, não por prop: o backend sabe a escolha da sessão (dict quente
  // + sidecar) e é a mesma resposta que traz a lista — pedir por prop obrigaria o Composer a
  // adivinhar o default da thread quando ninguém escolheu nada ainda.
  import { untrack } from 'svelte';
  import * as m from '../paraglide/messages';
  import Popover from './Popover.svelte';
  import { getCodexModels, setCodexModel } from '@hangar/core';
  import type { CodexModel } from '@hangar/core';

  interface Props {
    open: boolean;
    anchor: HTMLElement | null;
    sessionName: string;
    onApplied: (model: string, effort: string | null) => void;
    onClose: () => void;
  }
  let { open, anchor, sessionName, onApplied, onClose }: Props = $props();

  let models = $state<CodexModel[]>([]);
  let atual = $state<string | null>(null);
  let esforcoAtual = $state<string | null>(null);
  let loading = $state(false);
  let err = $state<string | null>(null);
  let aplicando = $state<string | null>(null);

  // Geração em voo, mesmo motivo do Pi e do Kimi: fechar durante um GET lento e reabrir deixaria a
  // resposta VELHA aterrissar por cima da nova.
  let carga = 0;

  async function load() {
    const minha = ++carga;
    err = null;
    loading = true;
    try {
      const res = await getCodexModels(sessionName);
      if (minha !== carga) return;
      models = res.models;
      atual = res.current.model;
      esforcoAtual = res.current.effort;
    } catch (e) {
      if (minha !== carga) return;
      err = e instanceof Error ? e.message : m.comum_falha_carregar_modelos();
    } finally {
      if (minha === carga) loading = false;
    }
  }

  $effect(() => {
    if (open) untrack(() => { aplicando = null; load(); });
  });

  // Trocar de modelo resolve o esforço pro default do NOVO — manter o antigo pediria um nível que
  // o modelo escolhido pode nem listar (era o que a folha já fazia no `pickModel`).
  async function escolher(md: CodexModel) {
    if (aplicando) return;
    aplicando = md.model;
    err = null;
    const esforco = md.model === atual
      ? (esforcoAtual ?? md.defaultEffort ?? null)
      : (md.defaultEffort ?? md.efforts[0]?.value ?? null);
    try {
      await setCodexModel(sessionName, md.model, esforco);
    } catch (e) {
      err = e instanceof Error ? e.message : m.comum_falha_aplicar();
      aplicando = null;
      return;
    }
    aplicando = null;
    onApplied(md.model, esforco);
    onClose();
  }
</script>

<Popover {open} {anchor} {onClose} width={300} ariaLabel={m.composer_modelo_codex()}>
  {#if err}
    <p class="err" role="alert">{err}</p>
  {/if}

  {#if loading && !models.length}
    <p class="vazio">{m.comum_carregando()}</p>
  {:else if !models.length}
    {#if !err}<p class="vazio">{m.comum_nenhum_modelo()}</p>{/if}
  {:else}
    <ul class="lista">
      {#each models as md (md.model)}
        <li>
          <button
            class="linha"
            class:ativa={atual === md.model}
            aria-pressed={atual === md.model}
            disabled={!!aplicando}
            data-foco={atual === md.model ? true : undefined}
            onclick={() => escolher(md)}
          >
            <span class="texto">
              <span class="nome">{md.displayName ?? md.model}</span>
              {#if md.description}<span class="meta">{md.description}</span>{/if}
            </span>
            {#if aplicando === md.model}
              <span class="tick" aria-hidden="true">…</span>
            {:else if atual === md.model}
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
    <p class="rodape">{m.modelo_vale_proxima()}</p>
  {/if}
</Popover>

<style>
  .err { color: var(--error); font-size: var(--text-xs); margin: 8px 10px 0; }
  .vazio { color: var(--text-muted); font-size: var(--text-sm); text-align: center; padding: 12px 0; }

  .lista { list-style: none; margin: 0; padding: 4px 0; overflow-y: auto; }

  .linha {
    display: flex;
    align-items: center;
    gap: 6px;
    width: 100%;
    padding: 6px 10px;
    background: transparent;
    border: none;
    color: var(--text-primary);
    text-align: left;
    cursor: pointer;
  }
  .linha:hover:not(:disabled) { background: var(--bg-hover); }
  /* (0,4,0) pra ganhar do hover (0,3,0), mesmo motivo do PiEffortPopover. */
  .linha.ativa:hover:not(:disabled) { background: var(--accent-dim); }
  .linha:disabled { cursor: default; }
  /* A linha escolhida se marca pelo FUNDO, nao pela cor do texto (contraste AA). */
  .linha.ativa { background: var(--accent-dim); color: var(--text-primary); }

  .texto { flex: 1; display: flex; flex-direction: column; gap: 2px; min-width: 0; }
  .nome { font-size: var(--text-sm); line-height: 1.3; }
  .meta { font-size: var(--text-xs); line-height: 1.3; color: var(--text-muted); }
  .tick { flex: none; color: var(--accent); }

  .rodape {
    font-size: var(--text-xs);
    color: var(--text-muted);
    padding: 6px 10px 4px;
    border-top: 1px solid var(--border-subtle);
  }
</style>
