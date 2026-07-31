<script lang="ts">
  import BottomSheet from './BottomSheet.svelte';
  import { getModelOptions, setEngineModel } from '../lib/api';
  import type { ModelEffortBody, ModelOption } from '../lib/api';

  // Sheet pra trocar modelo e esforco de raciocinio de uma sessao Claude Code. NADA e enviado ao
  // mexer: a selecao e local, so o botao aplica.
  //
  // A LISTA NAO E CHUMBADA AQUI. Ela era — 'default/opus/sonnet/haiku' — e envelheceu duas vezes:
  // o Fable entrou no picker e sumia desta tela, e numa sessao de MOTOR (Kimi, OmniRoute) os 4
  // nomes nem existem do outro lado. Agora o backend responde de onde a lista tem que vir:
  //   * kind 'claude' -> as linhas do picker do proprio Claude Code, lidas ao vivo;
  //   * kind 'engine' -> o /v1/models do provedor do motor, com busca (um deles tem 269 modelos).
  interface Props {
    open: boolean;
    sessionName: string;
    currentModel?: string | null;
    currentEffort?: string | null;
    onApply: (body: ModelEffortBody) => Promise<void> | void;
    onApplied?: (model: string, effort: string | null) => void;
    onClose: () => void;
  }
  let {
    open,
    sessionName,
    currentModel = null,
    currentEffort = null,
    onApply,
    onApplied,
    onClose,
  }: Props = $props();

  const MAX_ROWS = 40;   // teto de linhas desenhadas: 269 botoes travariam o celular

  // Niveis reais do /effort do Claude Code, ordenados Faster -> Smarter (6 paradas).
  // 'ultracode' e o topo. O picker do Opus expoe os 6; modelos menores expoem um subconjunto
  // (o backend acomoda) e o Haiku nao usa esforco.
  const EFFORTS = ['low', 'medium', 'high', 'xhigh', 'max', 'ultracode'];
  const EFFORT_DEFAULT = 3; // xhigh: parada neutra quando o nivel atual e desconhecido

  let kind = $state<'claude' | 'engine'>('claude');
  let models = $state<ModelOption[]>([]);
  let loading = $state(false);
  let err = $state<string | null>(null);
  let query = $state('');
  let selectedModel = $state<string | null>(null);
  let effortIdx = $state(EFFORT_DEFAULT);
  let applying = $state(false);

  // Casa o modelo atual (statusline: 'Opus 5', 'k3') com uma opcao da lista. Substring nos dois
  // sentidos: no picker o id e a keyword ('opus' dentro de 'Opus 5'); no motor o id e o proprio
  // nome que a statusline mostra ('k3').
  function matchCurrent(cur: string | null | undefined, opts: ModelOption[]): string | null {
    const c = cur?.trim().toLowerCase();
    if (!c) return null;
    const exato = opts.find((m) => m.id.toLowerCase() === c);
    if (exato) return exato.id;
    const ativo = opts.find((m) => m.active);
    if (ativo) return ativo.id;
    return opts.find((m) => m.id !== 'default' && c.includes(m.id.toLowerCase()))?.id ?? null;
  }

  // Mapeia o esforco atual pra parada do slider. Exato primeiro; senao prefixo (cobre
  // abreviacoes do statusline: 'med' -> medium, 'ultra' -> ultracode).
  function effortIndex(cur: string | null | undefined): number {
    if (!cur) return EFFORT_DEFAULT;
    const c = cur.trim().toLowerCase();
    const exact = EFFORTS.indexOf(c);
    if (exact >= 0) return exact;
    const pref = EFFORTS.findIndex((l) => l.startsWith(c));
    return pref >= 0 ? pref : EFFORT_DEFAULT;
  }

  // Geracao da busca em voo. O componente NAO e recriado entre um fechar e um reabrir da folha (o
  // BottomSheet so troca o markup), entao fechar durante um GET lento e reabrir dispara um segundo
  // load com o primeiro ainda pendente — e nada garante a ordem das respostas. Sem este carimbo, a
  // resposta VELHA aterrissava por cima da nova, trocando a lista por uma que o usuario abandonou.
  let carga = 0;

  async function load() {
    const minha = ++carga;
    err = null;
    loading = true;
    try {
      const res = await getModelOptions(sessionName);
      if (minha !== carga) return;   // chegou tarde: outra abertura ja mandou
      kind = res.kind;
      models = res.models;
      selectedModel = matchCurrent(currentModel, res.models);
      effortIdx = effortIndex(currentEffort ?? res.effort);
    } catch (e) {
      if (minha !== carga) return;
      // Sem lista nao ha troca de modelo — mas o esforco continua aplicavel, entao a folha segue
      // util em vez de virar uma tela de erro. O caso comum e 409 "a sessao esta trabalhando".
      models = [];
      // selectedModel TEM que cair junto: sem isso a escolha da abertura anterior ficava armada e
      // invisivel, e o "Aplicar" mandava trocar pra um modelo que a tela nao mostra mais — a
      // mensagem de erro dizia "nada vai acontecer" e o botao discordava.
      selectedModel = null;
      err = e instanceof Error ? e.message : 'Falha ao carregar modelos';
      effortIdx = effortIndex(currentEffort);
    } finally {
      if (minha === carga) loading = false;
    }
  }

  // Selecao LOCAL: re-sincroniza com o estado atual toda vez que a folha abre.
  $effect(() => {
    if (open) {
      query = '';
      applying = false;
      load();
    }
  });

  const buscavel = $derived(models.length > 8);   // lista curta nao precisa de campo de busca
  const casados = $derived.by(() => {
    const q = query.trim().toLowerCase();
    if (!q) return models;
    return models.filter((m) => `${m.id} ${m.name ?? ''} ${m.desc ?? ''}`.toLowerCase().includes(q));
  });
  const visiveis = $derived(casados.slice(0, MAX_ROWS));
  const escondidos = $derived(Math.max(0, casados.length - MAX_ROWS));

  const effortLevel = $derived(EFFORTS[effortIdx]);
  const effortFill = $derived((effortIdx / (EFFORTS.length - 1)) * 100);
  // Haiku nao usa esforco de raciocinio (o picker responde "Effort not supported").
  const semEsforco = $derived(kind === 'claude' && selectedModel === 'haiku');

  function rotulo(m: ModelOption): string {
    return m.name || m.id;
  }

  // Linha secundaria: no picker e a descricao do Claude Code; no motor, a janela real que o
  // provedor reporta (que varia por plano — por isso vem dele, nao de tabela). O id NAO se repete
  // aqui: no motor ele ja E o titulo da linha.
  function detalhe(m: ModelOption): string {
    if (kind === 'claude') return m.desc ?? '';
    const n = m.context_length;
    if (!n) return '';
    return n >= 1_000_000
      ? `${(n / 1_000_000).toFixed(n % 1_000_000 ? 1 : 0)}M de contexto`
      : `${Math.round(n / 1000)}k de contexto`;
  }

  function onEffortSlide(e: Event) {
    effortIdx = Number((e.currentTarget as HTMLInputElement).value); // local only
  }

  async function apply(scope: 'session' | 'default') {
    if (applying) return;
    applying = true;
    err = null;
    const efforto = semEsforco ? undefined : EFFORTS[effortIdx];
    try {
      if (kind === 'engine') {
        if (!selectedModel) {
          // So esforco numa sessao de motor: nao ha `/model <id>` a mandar, cai no picker.
          await onApply({ effort: efforto, scope: 'session' });
        } else {
          const res = await setEngineModel(sessionName, { model: selectedModel, effort: efforto });
          if (res.effort_error) {
            // O modelo pegou e o esforco nao: dizer "tudo certo" aqui seria reportar sucesso sobre
            // algo que ficou pela metade.
            onApplied?.(res.model, null);
            err = `Modelo trocado, mas o esforço não: ${res.effort_error}`;
            applying = false;
            return;
          }
          onApplied?.(res.model, efforto ?? null);
        }
      } else {
        await onApply({ model: selectedModel ?? undefined, effort: efforto, scope });
      }
    } catch (e) {
      // Falha real (rede/picker): mantem a folha aberta com o motivo, em vez de fechar calada.
      err = e instanceof Error ? e.message : 'Falha ao aplicar';
      applying = false;
      return;
    }
    applying = false;
    onClose();
  }
</script>

<BottomSheet {open} {onClose} ariaLabel="Modelo e esforço de raciocínio">
  <h2 class="sheet-title">Modelo</h2>

  {#if err}
    <p class="err">{err}</p>
  {/if}

  {#if loading && !models.length}
    <p class="empty">Carregando…</p>
  {:else if models.length}
    {#if buscavel}
      <input
        class="search"
        type="search"
        bind:value={query}
        placeholder="Buscar modelo…"
        aria-label="Buscar modelo"
      />
    {/if}

    <ul class="model-list" class:model-list--longa={buscavel}>
      {#each visiveis as m (m.id)}
        <li>
          <button
            class="model-row"
            class:active={selectedModel === m.id}
            aria-pressed={selectedModel === m.id}
            onclick={() => (selectedModel = m.id)}
          >
            <span class="model-text">
              <span class="model-name">{rotulo(m)}</span>
              {#if detalhe(m)}<span class="model-meta">{detalhe(m)}</span>{/if}
            </span>
            {#if selectedModel === m.id}
              <svg
                class="check"
                width="18"
                height="18"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                stroke-width="2.5"
                stroke-linecap="round"
                stroke-linejoin="round"
                aria-hidden="true"
              >
                <polyline points="20 6 9 17 4 12" />
              </svg>
            {/if}
          </button>
        </li>
      {/each}
    </ul>
    {#if escondidos}
      <p class="more">+{escondidos} — refine a busca</p>
    {:else if !casados.length}
      <p class="more">Nenhum modelo casa com “{query}”.</p>
    {/if}
  {/if}

  <div class="effort-head">
    <h3 class="section-label">Esforço de raciocínio</h3>
    <span class="effort-current">{semEsforco ? 'n/d' : effortLevel}</span>
  </div>
  <input
    class="range"
    type="range"
    min="0"
    max={EFFORTS.length - 1}
    step="1"
    value={effortIdx}
    style="--fill: {effortFill}%"
    oninput={onEffortSlide}
    disabled={semEsforco}
    aria-label="Esforço de raciocínio"
    aria-valuetext={effortLevel}
  />
  <div class="ends" aria-hidden="true">
    <span>Mais rápido</span>
    <span>Mais inteligente</span>
  </div>
  {#if semEsforco}
    <p class="effort-note">O Haiku não usa esforço de raciocínio.</p>
  {/if}

  <div class="actions">
    <button class="btn btn--primary" onclick={() => apply('session')} disabled={applying}>
      {applying ? 'Aplicando…' : 'Aplicar nesta sessão'}
    </button>
    {#if kind === 'claude'}
      <button class="btn btn--ghost" onclick={() => apply('default')} disabled={applying}>
        Salvar como padrão
      </button>
    {:else}
      <!-- Motor: o padrao da sessao nova mora no engines.json, nao no settings.json do Claude
           Code — salvar por aqui gravaria o id como default GLOBAL, inclusive pras sessoes da
           conta Anthropic, que nao conhecem esse modelo. Por isso o botao nao existe neste modo. -->
      <p class="effort-note">
        Vale só nesta sessão. Para mudar o padrão do motor, edite-o em Motores.
      </p>
    {/if}
  </div>
</BottomSheet>

<style>
  .sheet-title {
    font-size: var(--text-xl);
    font-weight: 600;
    color: var(--text-primary);
    margin-bottom: var(--space-4);
  }

  .err {
    color: var(--error);
    font-size: var(--text-sm);
    margin-bottom: var(--space-3);
  }

  .empty {
    color: var(--text-muted);
    font-size: var(--text-sm);
    text-align: center;
    padding: var(--space-4) 0;
  }

  /* Campo de busca: superficie propria (entrada de texto), logo --surface-inset — acompanha o
     slider de solidez em vez de virar retangulo chapado sobre o papel de parede. */
  .search {
    width: 100%;
    min-height: 44px;
    padding: var(--space-2) var(--space-3);
    margin-bottom: var(--space-3);
    border-radius: var(--radius-md);
    background: var(--surface-inset);
    color: var(--text-primary);
    font-size: var(--text-base);
  }

  /* ── Lista de modelos: rows grandes, tappaveis ─────────────────────────── */
  .model-list {
    list-style: none;
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
    margin-bottom: var(--space-5);
  }

  /* Catalogo de motor pode ter dezenas de linhas: rola dentro da folha em vez de empurrar o
     esforco e o botao Aplicar pra fora da tela. */
  .model-list--longa {
    max-height: 42vh;
    overflow-y: auto;
  }

  .model-row {
    width: 100%;
    min-height: 44px;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-3);
    padding: var(--space-2) var(--space-3);
    border-radius: var(--radius-md);
    text-align: left;
    background: transparent;
    transition: background 160ms var(--ease-out);
  }

  .model-row:active {
    background: var(--bg-hover);
  }

  .model-row.active {
    background: var(--accent-dim);
  }

  .model-text {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 2px;
    min-width: 0;
  }

  .model-name {
    font-size: var(--text-base);
    font-weight: 500;
    line-height: 1.3;
    color: var(--text-primary);
  }

  .model-meta {
    font-size: var(--text-sm);
    line-height: 1.3;
    color: var(--text-muted);
    overflow: hidden;
    text-overflow: ellipsis;
    max-width: 100%;
  }

  .more {
    font-size: var(--text-xs);
    color: var(--text-muted);
    text-align: center;
    margin: calc(-1 * var(--space-4)) 0 var(--space-4);
  }

  .check {
    color: var(--accent);
    flex-shrink: 0;
  }

  /* ── Esforco: slider Faster -> Smarter, 6 paradas (espelha o /effort do Claude) ── */
  .section-label {
    font-size: var(--text-sm);
    font-weight: 500;
    color: var(--text-secondary);
  }

  .effort-head {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: var(--space-3);
    margin-bottom: var(--space-3);
  }

  .effort-current {
    font-family: var(--font-mono);
    font-size: var(--text-sm);
    font-weight: 600;
    color: var(--accent);
    font-variant-numeric: tabular-nums;
  }

  /* Slider nativo estilizado: alvo de toque de 44px, trilho de 4px, polegar accent.
     --fill (inline) pinta a parte preenchida; o step=1 garante o snap nas 6 paradas. */
  .range {
    -webkit-appearance: none;
    appearance: none;
    width: 100%;
    height: 44px;
    min-height: 44px;
    background: transparent;
    cursor: pointer;
    display: block;
  }

  .range:focus {
    outline: none;
  }

  .range:disabled {
    opacity: 0.4;
    cursor: default;
  }

  .range::-webkit-slider-runnable-track {
    height: 4px;
    border-radius: var(--radius-full);
    background: linear-gradient(
      to right,
      var(--accent) var(--fill, 0%),
      var(--border-default) var(--fill, 0%)
    );
  }

  .range::-webkit-slider-thumb {
    -webkit-appearance: none;
    appearance: none;
    width: 22px;
    height: 22px;
    margin-top: -9px; /* centraliza no trilho de 4px */
    border-radius: var(--radius-full);
    background: var(--accent);
    border: 2px solid var(--bg-elevated);
    box-shadow: 0 1px 4px rgba(0, 0, 0, 0.45);
  }

  .range::-moz-range-track {
    height: 4px;
    border-radius: var(--radius-full);
    background: var(--border-default);
  }

  .range::-moz-range-progress {
    height: 4px;
    border-radius: var(--radius-full);
    background: var(--accent);
  }

  .range::-moz-range-thumb {
    width: 22px;
    height: 22px;
    border: 2px solid var(--bg-elevated);
    border-radius: var(--radius-full);
    background: var(--accent);
  }

  .range:focus-visible::-webkit-slider-thumb {
    box-shadow: 0 0 0 4px var(--accent-dim);
  }

  .range:focus-visible::-moz-range-thumb {
    box-shadow: 0 0 0 4px var(--accent-dim);
  }

  .ends {
    display: flex;
    justify-content: space-between;
    margin-top: var(--space-1);
  }

  .ends span {
    font-size: var(--text-xs);
    color: var(--text-muted);
  }

  .effort-note {
    font-size: var(--text-xs);
    color: var(--text-muted);
    margin-top: var(--space-2);
  }

  /* ── Acoes: aplicar so na sessao (primario) ou salvar como padrao (secundario) ── */
  .actions {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    margin-top: var(--space-5);
  }

  .btn {
    width: 100%;
    min-height: 48px;
    border-radius: var(--radius-md);
    font-size: var(--text-base);
    font-weight: 600;
    transition: background 180ms var(--ease-out), opacity 180ms var(--ease-out);
  }

  .btn:disabled {
    opacity: 0.55;
    cursor: default;
  }

  .btn--primary {
    background: var(--accent);
    color: #fff;
  }

  .btn--primary:active:not(:disabled) {
    background: var(--accent-press);
  }

  .btn--ghost {
    background: transparent;
    border: 1px solid var(--border-default);
    color: var(--text-secondary);
  }

  .btn--ghost:active:not(:disabled) {
    background: var(--bg-hover);
  }
</style>
