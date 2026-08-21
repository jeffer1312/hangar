<script lang="ts">
  // Nível de pensamento do Kimi, em pill própria ao lado da de modelo — a forma do PiEffortPopover.
  // Os níveis vêm do support_efforts do modelo ATUAL no config.toml (casa pelo display name da
  // statusline — modelo sem níveis, tipo kimi-for-coding, mostra a lista vazia em vez de oferecer
  // um no-op). A troca dirige a linha "Thinking (←→)" do picker e o read-back manda: quem pinta o
  // nível depois do clique é a resposta, nunca o que foi pedido.
  import { untrack } from 'svelte';
  import * as m from '../paraglide/messages';
  import Popover from './Popover.svelte';
  import { getKimiModels, setKimiModel } from '@hangar/core';

  interface Props {
    open: boolean;
    anchor: HTMLElement | null;
    sessionName: string;
    currentName: string | null;
    currentEffort: string | null;
    onApplied: (effort: string) => void;
    onClose: () => void;
  }
  let { open, anchor, sessionName, currentName, currentEffort, onApplied, onClose }: Props = $props();

  let levels = $state<string[]>([]);
  let loading = $state(false);
  let err = $state<string | null>(null);
  let aplicando = $state<string | null>(null);

  // Geração em voo, mesmo motivo do Pi: fechar durante um GET lento e reabrir deixaria a resposta
  // VELHA aterrissar por cima da nova.
  let carga = 0;

  async function load() {
    const minha = ++carga;
    err = null;
    loading = true;
    try {
      const res = await getKimiModels(sessionName);
      if (minha !== carga) return;
      const atual = (currentName ?? '').toLowerCase();
      const entry = res.models.find((md) => md.name.toLowerCase() === atual);
      levels = entry?.efforts ?? [];
    } catch (e) {
      if (minha !== carga) return;
      err = e instanceof Error ? e.message : m.modelo_niveis_erro();
    } finally {
      if (minha === carga) loading = false;
    }
  }

  // untrack: currentName vem da statusline e muda sozinho — ver PiEffortPopover.
  $effect(() => {
    if (open) untrack(() => { aplicando = null; load(); });
  });

  async function escolher(lv: string) {
    if (aplicando) return;
    aplicando = lv;
    err = null;
    try {
      const res = await setKimiModel(sessionName, { effort: lv });
      if (res.effort) onApplied(res.effort);
    } catch (e) {
      err = e instanceof Error ? e.message : m.comum_falha_aplicar();
      aplicando = null;
      return;
    }
    aplicando = null;
    onClose();
  }
</script>

<Popover {open} {anchor} {onClose} width={180} ariaLabel={m.composer_esforco()}>
  {#if err}
    <p class="err" role="alert">{err}</p>
  {/if}

  {#if loading && !levels.length}
    <p class="vazio">{m.comum_carregando()}</p>
  {:else if !levels.length}
    {#if !err}<p class="vazio">{m.modelo_sem_niveis()}</p>{/if}
  {:else}
    <ul class="lista">
      {#each levels as lv (lv)}
        <li>
          <button
            class="linha"
            class:ativa={currentEffort === lv}
            aria-pressed={currentEffort === lv}
            disabled={!!aplicando}
            data-foco={currentEffort === lv ? true : undefined}
            onclick={() => escolher(lv)}
          >
            <span class="nome">{lv}</span>
            {#if aplicando === lv}
              <span class="tick" aria-hidden="true">…</span>
            {:else if currentEffort === lv}
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
    font-size: var(--text-sm);
    text-align: left;
    cursor: pointer;
  }
  .linha:hover:not(:disabled) { background: var(--bg-hover); }
  /* (0,4,0) pra ganhar do hover (0,3,0), mesmo motivo do PiEffortPopover. */
  .linha.ativa:hover:not(:disabled) { background: var(--accent-dim); }
  .linha:disabled { cursor: default; }
  /* A linha escolhida se marca pelo FUNDO, nao pela cor do texto (contraste AA, ver PiEffortPopover). */
  .linha.ativa { background: var(--accent-dim); color: var(--text-primary); }

  .nome { flex: 1; text-transform: capitalize; }
  .tick { flex: none; color: var(--accent); }
</style>
