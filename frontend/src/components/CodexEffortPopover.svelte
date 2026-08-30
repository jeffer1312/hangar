<script lang="ts">
  // Nível de raciocínio do Codex, em pill própria ao lado da de modelo — a forma do
  // KimiEffortPopover. Os níveis são DO MODELO ATUAL (medido em 30/08/2026: gpt-5.6-sol aceita
  // `ultra`, gpt-5.5 não aceita nem `max`), então a lista sai do `efforts` daquela linha do
  // catálogo, nunca de uma lista fixa aqui.
  //
  // O POST exige o modelo junto (`CodexModelBody.model` é obrigatório): trocar só o esforço é
  // "mantenha o modelo, troque o nível", e é por isso que o mesmo GET traz os dois. Sem modelo
  // conhecido não há o que mandar, e a lista fica vazia em vez de oferecer um no-op.
  import { untrack } from 'svelte';
  import * as m from '../paraglide/messages';
  import Popover from './Popover.svelte';
  import { getCodexModels, setCodexModel } from '../lib/api';

  interface Props {
    open: boolean;
    anchor: HTMLElement | null;
    sessionName: string;
    onApplied: (effort: string) => void;
    onClose: () => void;
  }
  let { open, anchor, sessionName, onApplied, onClose }: Props = $props();

  let niveis = $state<{ value: string; description?: string | null }[]>([]);
  let modelo = $state<string | null>(null);
  let atual = $state<string | null>(null);
  let loading = $state(false);
  let err = $state<string | null>(null);
  let aplicando = $state<string | null>(null);

  // Geração em voo, mesmo motivo dos irmãos.
  let carga = 0;

  async function load() {
    const minha = ++carga;
    err = null;
    loading = true;
    try {
      const res = await getCodexModels(sessionName);
      if (minha !== carga) return;
      // Sem escolha explícita, o `current` traz o default da thread — é o modelo que o próximo
      // turno vai usar, e portanto de quem os níveis têm que sair. Quando nem isso existe (sessão
      // sem client vivo e sem sidecar), fica NULL: cair no primeiro do catálogo faria um toque
      // aqui trocar o MODELO sem ninguém pedir, e calado — a pill do lado seguiria dizendo
      // "Modelo". Quem resolve esse caso é a pill de modelo, não esta.
      modelo = res.current.model ?? null;
      const linha = res.models.find((md) => md.model === modelo);
      niveis = linha?.efforts ?? [];
      atual = res.current.effort ?? linha?.defaultEffort ?? null;
    } catch (e) {
      if (minha !== carga) return;
      err = e instanceof Error ? e.message : m.modelo_niveis_erro();
    } finally {
      if (minha === carga) loading = false;
    }
  }

  $effect(() => {
    if (open) untrack(() => { aplicando = null; load(); });
  });

  async function escolher(lv: string) {
    if (aplicando || !modelo) return;
    aplicando = lv;
    err = null;
    try {
      await setCodexModel(sessionName, modelo, lv);
    } catch (e) {
      err = e instanceof Error ? e.message : m.comum_falha_aplicar();
      aplicando = null;
      return;
    }
    aplicando = null;
    onApplied(lv);
    onClose();
  }
</script>

<Popover {open} {anchor} {onClose} width={200} ariaLabel={m.composer_esforco_raciocinio()}>
  {#if err}
    <p class="err" role="alert">{err}</p>
  {/if}

  {#if loading && !niveis.length}
    <p class="vazio">{m.comum_carregando()}</p>
  {:else if !niveis.length}
    {#if !err}<p class="vazio">{m.modelo_sem_niveis()}</p>{/if}
  {:else}
    <ul class="lista">
      {#each niveis as lv (lv.value)}
        <li>
          <button
            class="linha"
            class:ativa={atual === lv.value}
            aria-pressed={atual === lv.value}
            disabled={!!aplicando}
            data-foco={atual === lv.value ? true : undefined}
            onclick={() => escolher(lv.value)}
          >
            <span class="texto">
              <span class="nome">{lv.value}</span>
              <!-- A descrição vem do provedor ("Fast responses with lighter reasoning") e a folha
                   antiga já a mostrava. Pi e Kimi não têm porque a fonte deles não manda; aqui
                   manda, e escondê-la seria perder conteúdo numa mudança que é só de formato. -->
              {#if lv.description}<span class="meta">{lv.description}</span>{/if}
            </span>
            {#if aplicando === lv.value}
              <span class="tick" aria-hidden="true">…</span>
            {:else if atual === lv.value}
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
  /* A linha escolhida se marca pelo FUNDO, nao pela cor do texto (contraste AA). */
  .linha.ativa { background: var(--accent-dim); color: var(--text-primary); }

  .texto { flex: 1; display: flex; flex-direction: column; gap: 2px; min-width: 0; }
  .nome { text-transform: capitalize; }
  .meta { font-size: var(--text-xs); line-height: 1.3; color: var(--text-muted); text-transform: none; }
  .tick { flex: none; color: var(--accent); }
</style>
