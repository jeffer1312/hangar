<script lang="ts">
  // Esforco de raciocinio do Claude, em pill propria — irmao do PiEffortPopover. Antes era um
  // SLIDER dentro da folha de modelo; virou lista pelo mesmo motivo da referencia (opencode): o
  // valor e discreto, tem nome, e escolher por nome erra menos que mirar uma parada de slider.
  //
  // Os seis niveis sao os do /effort do Claude Code, ordenados do mais rapido ao mais inteligente.
  // O picker do Opus expoe os seis; modelos menores expoem um subconjunto (o backend acomoda) e o
  // Haiku nao usa esforco — por isso a pill some quando o modelo atual e Haiku (ver Composer).
  import Popover from './Popover.svelte';
  import type { ModelEffortBody } from '../lib/api';

  const EFFORTS = ['low', 'medium', 'high', 'xhigh', 'max', 'ultracode'];

  interface Props {
    open: boolean;
    anchor: HTMLElement | null;
    currentEffort?: string | null;
    onApply: (body: ModelEffortBody) => Promise<void> | void;
    onClose: () => void;
  }
  let { open, anchor, currentEffort = null, onApply, onClose }: Props = $props();

  let err = $state<string | null>(null);
  let aplicando = $state<string | null>(null);

  // Casa o nivel atual com a lista. Prefixo cobre a abreviacao da statusline ('med' -> medium,
  // 'ultra' -> ultracode).
  const atual = $derived.by(() => {
    const c = currentEffort?.trim().toLowerCase();
    if (!c) return null;
    return EFFORTS.find((l) => l === c) ?? EFFORTS.find((l) => l.startsWith(c)) ?? null;
  });

  // `aplicando` zera junto com o erro: fechar com pedido em voo travava a lista na reabertura.
  $effect(() => {
    if (open) { err = null; aplicando = null; }
  });

  async function escolher(lv: string) {
    if (aplicando) return;
    aplicando = lv;
    err = null;
    try {
      await onApply({ effort: lv, scope: 'session' });
    } catch (e) {
      err = e instanceof Error ? e.message : 'Falha ao aplicar';
      aplicando = null;
      return;
    }
    aplicando = null;
    onClose();
  }
</script>

<Popover {open} {anchor} {onClose} width={200} ariaLabel="Esforço de raciocínio">
  {#if err}
    <p class="err" role="alert">{err}</p>
  {/if}

  <ul class="lista">
    {#each EFFORTS as lv (lv)}
      <li>
        <button
          class="linha"
          class:ativa={atual === lv}
          aria-pressed={atual === lv}
          disabled={!!aplicando}
          data-foco={atual === lv ? true : undefined}
          onclick={() => escolher(lv)}
        >
          <span class="nome">{lv}</span>
          {#if aplicando === lv}
            <span class="tick" aria-hidden="true">…</span>
          {:else if atual === lv}
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
  <p class="dica">Do mais rápido ao mais inteligente.</p>
</Popover>

<style>
  .err { color: var(--error); font-size: var(--text-xs); margin: 8px 10px 0; }
  .lista { list-style: none; margin: 0; padding: 4px 0; overflow-y: auto; }

  .linha {
    display: flex; align-items: center; gap: 6px; width: 100%;
    padding: 6px 10px; background: transparent; border: none;
    color: var(--text-primary); font-size: var(--text-sm); text-align: left; cursor: pointer;
  }
  .linha:hover:not(:disabled) { background: var(--bg-hover); }
  /* (0,4,0) pra ganhar do hover (0,3,0): desde que o marcador de selecao virou FUNDO, passar o
     mouse por cima da linha atual apagava a marcacao e so o tique segurava. */
  .linha.ativa:hover:not(:disabled) { background: var(--accent-dim); }
  .linha:disabled { cursor: default; }
  /* A linha escolhida se marca pelo FUNDO, nao pela cor do texto: --accent sobre o papel do tema
     claro da 4,0:1, abaixo dos 4,5:1 que o AA pede pra texto de 14px (a Task 6 ja reprovou um
     4,34:1 pelo mesmo motivo). Fundo tingido + texto normal passa folgado, e e o mesmo desenho que
     a folha antiga usava. O tique continua em --accent: e grafico, e grafico pede 3:1. */
  .linha.ativa { background: var(--accent-dim); color: var(--text-primary); }

  .nome { flex: 1; text-transform: capitalize; }
  .tick { flex: none; color: var(--accent); }

  .dica {
    font-size: var(--text-xs); color: var(--text-muted);
    padding: 6px 10px 8px; margin: 0; border-top: 1px solid var(--border-subtle);
  }
</style>
