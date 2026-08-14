<script lang="ts">
  // Nivel de raciocinio do Pi, em pill propria ao lado da de modelo — como no opencode, onde o
  // nivel tem seu proprio botao e sua propria lista curta. Antes isto era uma secao dentro da folha
  // de modelo; separar deixa a troca de nivel a um clique, sem passar pela lista de 390 modelos.
  //
  // Os niveis NAO sao fixos: cada modelo aceita um conjunto (`levels`, do sidecar), e depois de
  // trocar de modelo eles mudam. Por isso a lista e buscada ao abrir, e nao chumbada.
  import { untrack } from 'svelte';
  import * as m from '../paraglide/messages';
  import Popover from './Popover.svelte';
  import { getPiModels, setPiModel } from '../lib/api';

  interface Props {
    open: boolean;
    anchor: HTMLElement | null;
    sessionName: string;
    onApplied: (effort: string | null) => void;
    onClose: () => void;
  }
  let { open, anchor, sessionName, onApplied, onClose }: Props = $props();

  let levels = $state<string[]>([]);
  let atual = $state<string | null>(null);
  let loading = $state(false);
  let err = $state<string | null>(null);
  let aplicando = $state<string | null>(null);

  // Geracao em voo: fechar durante um GET lento e reabrir deixaria a resposta VELHA aterrissar
  // por cima da nova (o componente nao e recriado entre uma abertura e outra).
  let carga = 0;

  async function load() {
    const minha = ++carga;
    err = null;
    loading = true;
    try {
      const res = await getPiModels(sessionName);
      if (minha !== carga) return;
      levels = res.levels ?? [];
      atual = res.thinking;
    } catch (e) {
      if (minha !== carga) return;
      err = e instanceof Error ? e.message : m.modelo_niveis_erro();
    } finally {
      if (minha === carga) loading = false;
    }
  }

  // untrack: ver ClaudeModelPopover — efeito que depende de prop volatil recarrega no meio do uso.
  // `aplicando` zera junto: fechar com pedido em voo travava a lista na abertura seguinte.
  $effect(() => {
    if (open) untrack(() => { aplicando = null; load(); });
  });

  // Read-back manda: o Pi clampa o nivel pro que o modelo suporta, entao quem pinta a lista depois
  // do clique e a resposta, nunca o que foi pedido.
  async function escolher(lv: string) {
    if (aplicando) return;
    aplicando = lv;
    err = null;
    try {
      const res = await setPiModel(sessionName, { effort: lv });
      levels = res.levels;
      atual = res.thinking;
      onApplied(res.thinking);
    } catch (e) {
      err = e instanceof Error ? e.message : m.comum_falha_aplicar();
      aplicando = null;
      return;
    }
    aplicando = null;
    onClose();
  }
</script>

<Popover {open} {anchor} {onClose} width={180} ariaLabel={m.composer_nivel_raciocinio_pi()}>
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
    <p class="dica">{m.modelo_vale_atual()}</p>
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
    font-size: var(--text-xs);
    color: var(--text-muted);
    padding: 6px 10px 8px;
    margin: 0;
    border-top: 1px solid var(--border-subtle);
  }
</style>
