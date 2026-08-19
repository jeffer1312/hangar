<script lang="ts">
  // Seletor de modelo do Kimi ancorado na pill do composer. Irmão do PiModelPopover (caixa
  // compacta, busca no topo, clique aplica), mas com duas diferenças do mecanismo dele:
  //   * fonte única: o catálogo é o config.toml (GET /kimi/models) — não há sidecar de extensão
  //     nem `kimi --list-models` pra enriquecer;
  //   * o "atual" chega por PROP (display name da statusline), porque o backend não tem fonte
  //     barata do modelo vivo — e o nome repete entre providers (K3 no apikey e no kimi-code),
  //     então a marcação é por nome mesmo, com o alias desempatando na linha.
  import { untrack } from 'svelte';
  import * as m from '../paraglide/messages';
  import Popover from './Popover.svelte';
  import { getKimiModels, setKimiModel } from '../lib/api';
  import type { KimiModel } from '../lib/api';

  interface Props {
    open: boolean;
    anchor: HTMLElement | null;
    sessionName: string;
    currentName: string | null;
    onApplied: (model: string) => void;
    onClose: () => void;
  }
  let { open, anchor, sessionName, currentName, onApplied, onClose }: Props = $props();

  const MAX_ROWS = 40;   // teto de linhas desenhadas, igual ao do Pi

  let models = $state<KimiModel[]>([]);
  let loading = $state(false);
  let err = $state<string | null>(null);
  let aplicando = $state<string | null>(null);   // alias em voo, pra marcar a linha
  let query = $state('');

  // Geração da busca em voo, mesmo motivo do Pi: fechar durante um GET lento e reabrir deixaria
  // a resposta VELHA aterrissar por cima da nova.
  let carga = 0;

  async function load() {
    const minha = ++carga;
    err = null;
    loading = true;
    try {
      const res = await getKimiModels(sessionName);
      if (minha !== carga) return;
      models = res.models;
    } catch (e) {
      if (minha !== carga) return;
      err = e instanceof Error ? e.message : m.comum_falha_carregar_modelos();
    } finally {
      if (minha === carga) loading = false;
    }
  }

  // untrack, mesmo motivo do Pi: currentName vem da statusline e muda sozinho — sem isolar, o
  // efeito recarregava a lista no meio do uso.
  $effect(() => {
    if (open) untrack(() => { query = ''; aplicando = null; load(); });
  });

  function casa(md: KimiModel, q: string) {
    return `${md.alias} ${md.name}`.toLowerCase().includes(q);
  }

  const achados = $derived.by(() => {
    const q = query.trim().toLowerCase();
    return q ? models.filter((md) => casa(md, q)) : models;
  });
  const filtered = $derived(achados.slice(0, MAX_ROWS));
  const hiddenCount = $derived(Math.max(0, achados.length - MAX_ROWS));

  // Agrupa por provider preservando a ordem do catálogo — reordenar esconderia a ordem da fonte.
  const agrupado = $derived.by(() => {
    const mapa = new Map<string, KimiModel[]>();
    for (const md of filtered) {
      if (!mapa.has(md.provider)) mapa.set(md.provider, []);
      mapa.get(md.provider)!.push(md);
    }
    return [...mapa.entries()];
  });

  const atual = $derived((currentName ?? '').toLowerCase());
  function isAtual(md: KimiModel) {
    return !!atual && md.name.toLowerCase() === atual;
  }

  // Clique aplica. O read-back manda: o backend confirma pela linha "Switched to …" e devolve o
  // que FICOU — fechar sobre escolha não confirmada pintaria uma troca que não pegou.
  async function escolher(md: KimiModel) {
    if (aplicando) return;
    aplicando = md.alias;
    err = null;
    try {
      const res = await setKimiModel(sessionName, md.alias);
      onApplied(res.current.name);
    } catch (e) {
      err = e instanceof Error ? e.message : m.comum_falha_aplicar();
      aplicando = null;
      return;
    }
    aplicando = null;
    onClose();
  }
</script>

<Popover {open} {anchor} {onClose} width={320} ariaLabel={m.composer_modelo()}>
  <div class="busca-wrap">
    <svg class="lupa" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor"
      stroke-width="2" stroke-linecap="round" aria-hidden="true">
      <circle cx="11" cy="11" r="7" /><path d="m20 20-3.5-3.5" />
    </svg>
    <input
      class="busca"
      type="search"
      data-foco
      bind:value={query}
      placeholder={m.comum_buscar_modelos()}
      aria-label={m.comum_buscar_modelo()}
    />
  </div>

  {#if err}
    <p class="err" role="alert">{err}</p>
  {/if}

  {#if loading && !models.length}
    <p class="vazio">{m.comum_carregando()}</p>
  {:else if !models.length}
    {#if !err}<p class="vazio">{m.comum_nenhum_modelo()}</p>{/if}
  {:else if !filtered.length}
    <p class="vazio">{m.comum_nada_encontrado()}</p>
  {:else}
    <div class="lista-scroll">
      {#each agrupado as [prov, itens] (prov)}
        <p class="grupo">{prov}</p>
        <ul class="lista">
          {#each itens as md (md.alias)}
            <li>
              <button
                class="linha"
                class:ativa={isAtual(md)}
                aria-pressed={isAtual(md)}
                disabled={!!aplicando}
                onclick={() => escolher(md)}
              >
                <span class="nome">{md.name}</span>
                {#if md.context_length}
                  <span class="tag">{Math.round(md.context_length / 1000)}K</span>
                {/if}
                {#if aplicando === md.alias}
                  <span class="tick" aria-hidden="true">…</span>
                {:else if isAtual(md)}
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
      {/each}
    </div>
    {#if hiddenCount}
      <p class="mais">{m.modelo_refine_busca({ n: hiddenCount })}</p>
    {/if}
  {/if}
</Popover>

<style>
  .busca-wrap {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 8px 10px;
    border-bottom: 1px solid var(--border-subtle);
  }
  .lupa { color: var(--text-muted); flex: none; }
  .busca {
    flex: 1;
    min-width: 0;
    background: transparent;
    border: none;
    outline: none;
    color: var(--text-primary);
    font-size: var(--text-sm);
  }
  .busca::placeholder { color: var(--text-muted); }

  .err { color: var(--error); font-size: var(--text-xs); margin: 8px 10px 0; }
  .vazio { color: var(--text-muted); font-size: var(--text-sm); text-align: center; padding: 14px 0; }

  .lista-scroll { overflow-y: auto; padding: 4px 0; }

  /* Cabecalho de grupo: <p>, nao heading — a caixa e um dialog sem hierarquia de secao, e um h4
     solto aqui viraria salto de nivel na arvore de acessibilidade. */
  .grupo {
    font-size: var(--text-xs);
    color: var(--text-muted);
    padding: 6px 10px 2px;
    margin: 0;
  }

  .lista { list-style: none; margin: 0; padding: 0; }

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
  /* (0,4,0) pra ganhar do hover (0,3,0), mesmo motivo do PiModelPopover. */
  .linha.ativa:hover:not(:disabled) { background: var(--accent-dim); }
  .linha:disabled { cursor: default; }
  /* A linha escolhida se marca pelo FUNDO, nao pela cor do texto (contraste AA, ver PiModelPopover). */
  .linha.ativa { background: var(--accent-dim); color: var(--text-primary); }

  .nome { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }

  .tag {
    flex: none;
    font-size: var(--text-xs);
    color: var(--text-muted);
    border: 1px solid var(--border-subtle);
    border-radius: var(--radius-sm);
    padding: 0 5px;
  }

  .tick { flex: none; color: var(--accent); }

  .mais {
    font-size: var(--text-xs);
    color: var(--text-muted);
    text-align: center;
    padding: 6px 0 8px;
    margin: 0;
    border-top: 1px solid var(--border-subtle);
  }
</style>
