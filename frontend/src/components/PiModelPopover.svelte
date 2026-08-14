<script lang="ts">
  // Seletor de modelo do Pi ancorado na pill do composer. Substitui o PiModelSheet (painel de
  // altura cheia) pela forma que o usuario pediu, com o opencode como referencia: caixa compacta
  // sobre a pill, busca no topo, lista agrupada por provedor, tique no atual, e CLIQUE APLICA —
  // sem botao Aplicar. O nivel de raciocinio saiu daqui: virou pill propria (PiEffortPopover).
  import { untrack } from 'svelte';
  import * as m from '../paraglide/messages';
  import Popover from './Popover.svelte';
  import { getPiModels, setPiModel, modelOptions } from '../lib/api';
  import type { PiModel } from '../lib/types';

  interface Props {
    open: boolean;
    anchor: HTMLElement | null;
    sessionName: string;
    onApplied: (model: string, effort: string | null) => void;
    onClose: () => void;
  }
  let { open, anchor, sessionName, onApplied, onClose }: Props = $props();

  const MAX_ROWS = 40;   // teto de linhas desenhadas: 390 botoes travariam o celular

  let models = $state<PiModel[]>([]);
  let loading = $state(false);
  let err = $state<string | null>(null);
  let aplicando = $state<string | null>(null);   // provider/id em voo, pra marcar a linha
  let query = $state('');
  let selected = $state<PiModel | null>(null);

  // Geracao da busca em voo: o componente NAO e recriado entre fechar e reabrir, entao fechar
  // durante um GET lento e reabrir dispara um segundo load com o primeiro pendente — sem este
  // carimbo, a resposta VELHA aterrissa por cima da nova.
  let carga = 0;

  async function load() {
    const minha = ++carga;
    err = null;
    loading = true;
    try {
      // allSettled, NAO all: o sidecar e leitura de arquivo local e quase nao falha; o catalogo e
      // subprocess Node. Com Promise.all, a falha do catalogo derrubaria a lista inteira.
      const [est, cat] = await Promise.allSettled([getPiModels(sessionName), modelOptions('pi')]);
      if (minha !== carga) return;
      if (est.status === 'rejected') throw est.reason;
      // O SIDECAR e o conjunto; o catalogo so enriquece. Quem valida o apply e o check_known contra
      // o catalogo DO SIDECAR — oferecer id que so o `pi --list-models` conhece e 422 na cara do
      // usuario (medido: 390 vs 388, dois modelos so na lista nova).
      const extra = new Map<string, { context?: string; images?: boolean }>();
      if (cat.status === 'fulfilled')
        for (const c of cat.value.models)
          if (c.provider) extra.set(`${c.provider}/${c.id}`, { context: c.context, images: c.images });
      models = est.value.models.map((m) => ({ ...m, ...(extra.get(`${m.provider}/${m.id}`) ?? {}) }));
      selected = est.value.current
        ? models.find((m) => m.provider === est.value.current!.provider && m.id === est.value.current!.id) ?? null
        : null;
    } catch (e) {
      if (minha !== carga) return;
      err = e instanceof Error ? e.message : m.comum_falha_carregar_modelos();
    } finally {
      if (minha === carga) loading = false;
    }
  }

  // untrack: o load() le props que MUDAM SOZINHAS (o modelo atual vem da statusline e atualiza a
  // cada tick). Sem isolar, o efeito virava dependente delas, recarregava a lista no meio do uso e
  // REESCREVIA a selecao do usuario com o modelo atual — clicar numa linha nao pegava.
  // `aplicando` volta a null ao reabrir: fechar com um pedido em voo deixava a lista `disabled`
  // na abertura seguinte, sem nada explicando.
  $effect(() => {
    if (open) untrack(() => { query = ''; aplicando = null; load(); });
  });

  function casa(m: PiModel, q: string) {
    return `${m.provider}/${m.id} ${m.name ?? m.id}`.toLowerCase().includes(q);
  }

  const achados = $derived.by(() => {
    const q = query.trim().toLowerCase();
    return q ? models.filter((m) => casa(m, q)) : models;
  });
  const filtered = $derived(achados.slice(0, MAX_ROWS));
  const hiddenCount = $derived(Math.max(0, achados.length - MAX_ROWS));

  // Agrupa preservando a ordem que a fonte mandou — reordenar esconderia a ordem de relevancia.
  const agrupado = $derived.by(() => {
    const mapa = new Map<string, PiModel[]>();
    for (const m of filtered) {
      if (!mapa.has(m.provider)) mapa.set(m.provider, []);
      mapa.get(m.provider)!.push(m);
    }
    return [...mapa.entries()];
  });

  function same(a: PiModel | null, b: PiModel) {
    return !!a && a.provider === b.provider && a.id === b.id;
  }

  // Clique aplica. O read-back manda: o Pi clampa o nivel pro que o modelo aceita, e 200 sem
  // `current` significa que NAO da pra afirmar que a troca pegou — nesse caso a caixa fica aberta
  // com o aviso, em vez de fechar pintando uma escolha nao confirmada.
  async function escolher(md: PiModel) {
    if (aplicando) return;
    const chave = `${md.provider}/${md.id}`;
    aplicando = chave;
    err = null;
    try {
      const res = await setPiModel(sessionName, { provider: md.provider, model: md.id });
      if (!res.current) {
        err = m.modelo_confirmar_erro();
        aplicando = null;
        return;
      }
      selected = md;
      onApplied(res.current.name || res.current.id, res.thinking);
    } catch (e) {
      err = e instanceof Error ? e.message : m.comum_falha_aplicar();
      aplicando = null;
      return;
    }
    aplicando = null;
    onClose();
  }
</script>

<Popover {open} {anchor} {onClose} width={320} ariaLabel={m.composer_modelo_pi()}>
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
          {#each itens as md (md.provider + '/' + md.id)}
            <li>
              <button
                class="linha"
                class:ativa={same(selected, md)}
                aria-pressed={same(selected, md)}
                disabled={!!aplicando}
                onclick={() => escolher(md)}
              >
                <span class="nome">{md.name ?? md.id}</span>
                {#if md.context}<span class="tag">{md.context}</span>{/if}
                {#if md.images}<span class="tag tag--olho" title={m.modelo_suporta_imagens()}>👁</span>{/if}
                {#if aplicando === `${md.provider}/${md.id}`}
                  <span class="tick" aria-hidden="true">…</span>
                {:else if same(selected, md)}
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
  /* (0,4,0) pra ganhar do hover (0,3,0): desde que o marcador de selecao virou FUNDO, passar o
     mouse por cima da linha atual apagava a marcacao e so o tique segurava. */
  .linha.ativa:hover:not(:disabled) { background: var(--accent-dim); }
  .linha:disabled { cursor: default; }
  /* A linha escolhida se marca pelo FUNDO, nao pela cor do texto: --accent sobre o papel do tema
     claro da 4,0:1, abaixo dos 4,5:1 que o AA pede pra texto de 14px (a Task 6 ja reprovou um
     4,34:1 pelo mesmo motivo). Fundo tingido + texto normal passa folgado, e e o mesmo desenho que
     a folha antiga usava. O tique continua em --accent: e grafico, e grafico pede 3:1. */
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
  .tag--olho { border: none; padding: 0; }

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
