<script lang="ts">
  // Seletor de modelo do Claude ancorado na pill do composer — irmao do PiModelPopover, mesma
  // referencia (opencode): caixa compacta sobre a pill, busca quando a lista e longa, tique no
  // atual. O esforco saiu daqui: virou pill propria (ClaudeEffortPopover).
  //
  // DIFERENCA PRO PI, medida na tela: aqui clique NAO aplica. A troca do Claude dispara o PICKER
  // INTERATIVO no terminal ("Switch between Claude models: 1. Default 2. Opus ...") e o backend
  // dirige esse picker; leva tempo e o usuario ve a pergunta na conversa. Aplicar no clique fechava
  // a caixa no meio disso, sem ele conseguir escolher. Entao: clique SELECIONA, botao APLICA.
  //
  // A LISTA NAO E CHUMBADA. Ela era ('default/opus/sonnet/haiku') e envelheceu duas vezes: o Fable
  // entrou no picker e sumia desta tela, e numa sessao de MOTOR os quatro nomes nem existem do
  // outro lado. Quem responde de onde vem e o backend: kind 'claude' = linhas do picker lidas ao
  // vivo; kind 'engine' = /v1/models do provedor (um deles tem 269 modelos).
  import { untrack } from 'svelte';
  import * as m from '../paraglide/messages';
  import Popover from './Popover.svelte';
  import { getModelOptions, setEngineModel } from '@hangar/core';
  import type { ModelEffortBody, ModelOption } from '@hangar/core';

  interface Props {
    open: boolean;
    anchor: HTMLElement | null;
    sessionName: string;
    currentModel?: string | null;
    currentEffort?: string | null;
    onApply: (body: ModelEffortBody) => Promise<void> | void;
    onApplied?: (model: string, effort: string | null) => void;
    /** Falha que chegou DEPOIS da caixa fechar — quem mostra e o composer, na linha de erro dele. */
    onFail?: (msg: string) => void;
    onClose: () => void;
    /** Modo de permissão atual + abridor do popover dele. A permissão saiu da fileira do composer
        (a palavra do modo estourava a linha no celular) e virou uma linha deste seletor — é aqui
        que se mexe em modelo, então é aqui que se mexe no modo. */
    permCurrent?: string | null;
    onOpenPermission?: () => void;
  }
  let {
    open, anchor, sessionName, currentModel = null, currentEffort = null,
    onApply, onApplied, onFail, onClose,
    permCurrent = null, onOpenPermission,
  }: Props = $props();

  const MAX_ROWS = 40;   // teto de linhas desenhadas: 269 botoes travariam o celular

  let kind = $state<'claude' | 'engine'>('claude');
  let models = $state<ModelOption[]>([]);
  let loading = $state(false);
  let err = $state<string | null>(null);
  let query = $state('');
  let atual = $state<string | null>(null);        // o que esta valendo na sessao
  let escolhido = $state<string | null>(null);    // selecao local, ainda nao aplicada
  let aplicando = $state(false);

  // Casa o modelo atual (statusline: 'Fable 5', 'Opus 5', 'k3') com uma linha da lista.
  //
  // ORDEM IMPORTA, e a do sheet estava errada: o `active` que o backend marca vinha ANTES do
  // casamento por nome, e como ele envelhece (o picker guarda o ultimo escolhido por ali), a sessao
  // rodando em `Fable 5` aparecia com o tique no Opus. Agora o que a statusline diz AGORA ganha;
  // `active` so entra quando o nome nao casa com nada.
  function matchCurrent(cur: string | null | undefined, opts: ModelOption[]): string | null {
    const c = cur?.trim().toLowerCase();
    if (!c) return opts.find((m) => m.active)?.id ?? null;
    const exato = opts.find((m) => m.id.toLowerCase() === c);
    if (exato) return exato.id;
    // Substring nos dois sentidos: no picker o id e a keyword ('fable' dentro de 'Fable 5'); no
    // motor o id e o proprio nome que a statusline mostra ('k3').
    //
    // DUAS linhas podem dividir a mesma palavra — 'opus' e 'opus[1m]'. O que decide entre elas e a
    // statusline citar a janela de 1M ou nao ('Opus5·1M' vs 'Opus 5'); sem isso, quem roda no 1M
    // via o tique no Opus normal, que foi o que apareceu na tela.
    const querMilhao = c.includes('1m');
    const base = (id: string) => id.replace(/\[[^\]]*\]$/, '').toLowerCase();
    const candidatos = opts.filter((m) => m.id !== 'default' && c.includes(base(m.id)));
    const porJanela = candidatos.find(
      (m) => m.id.toLowerCase().endsWith('[1m]') === querMilhao,
    );
    if (porJanela) return porJanela.id;
    if (candidatos.length) return candidatos[0].id;
    return opts.find((m) => m.active)?.id ?? null;
  }

  // Geracao da busca em voo: o componente NAO e recriado entre fechar e reabrir, entao fechar
  // durante um GET lento e reabrir dispara um segundo load com o primeiro pendente — sem este
  // carimbo, a resposta VELHA aterrissa por cima da nova.
  let carga = 0;

  async function load() {
    const minha = ++carga;
    err = null;
    loading = true;
    try {
      const res = await getModelOptions(sessionName);
      if (minha !== carga) return;
      kind = res.kind;
      models = res.models;
      atual = matchCurrent(currentModel, res.models);
      escolhido = atual;
    } catch (e) {
      if (minha !== carga) return;
      models = [];
      atual = null;
      escolhido = null;
      err = e instanceof Error ? e.message : m.comum_falha_carregar_modelos();
    } finally {
      if (minha === carga) loading = false;
    }
  }

  // untrack: o load() le props que MUDAM SOZINHAS (o modelo atual vem da statusline e atualiza a
  // cada tick). Sem isolar, o efeito virava dependente delas, recarregava a lista no meio do uso e
  // REESCREVIA a selecao do usuario com o modelo atual — clicar numa linha nao pegava.
  // `aplicando` volta a false ao reabrir: fechar (Esc/fundo) com um pedido em voo deixava a lista
  // inteira `disabled` na proxima abertura, sem nada explicando — a caixa abria morta.
  $effect(() => {
    if (open) untrack(() => { query = ''; aplicando = false; load(); });
  });

  const casados = $derived.by(() => {
    const q = query.trim().toLowerCase();
    if (!q) return models;
    return models.filter((m) => `${m.id} ${m.name ?? ''} ${m.desc ?? ''}`.toLowerCase().includes(q));
  });
  const visiveis = $derived(casados.slice(0, MAX_ROWS));
  const escondidos = $derived(Math.max(0, casados.length - MAX_ROWS));
  const buscavel = $derived(models.length > 8);   // lista curta nao precisa de campo de busca

  function rotulo(m: ModelOption): string {
    return m.name || m.id;
  }

  // Linha secundaria: no picker e a descricao do Claude Code; no motor, a janela real que o
  // provedor reporta. O id NAO se repete no motor — la ele ja E o titulo.
  function detalhe(md: ModelOption): string {
    if (kind === 'claude') return md.desc ?? '';
    const n = md.context_length;
    if (!n) return '';
    return n >= 1_000_000
      ? m.modelo_ctx_m({ n: (n / 1_000_000).toFixed(n % 1_000_000 ? 1 : 0) })
      : m.modelo_ctx_k({ n: Math.round(n / 1000) });
  }

  // Aplica NESTA SESSAO o que esta selecionado. "Salvar como padrão" e outro destino (escreve a
  // preferencia pra sessoes novas) e por isso e botao proprio, nao um clique de linha.
  //
  // FECHA ANTES DE ESPERAR, de proposito. Trocar de modelo no Claude abre o picker no TERMINAL e,
  // as vezes, uma confirmacao (a janela de contexto muda) que quem aceita e o USUARIO, na conversa.
  // Ficar aberto com "Aplicando…" tapava justamente a pergunta que ele precisa responder. Entao a
  // caixa sai da frente assim que o pedido parte; o pill e a statusline contam o desfecho.
  async function aplicar(scope: 'session' | 'default') {
    const alvo = models.find((x) => x.id === escolhido);
    if (!alvo || aplicando) return;
    aplicando = true;
    err = null;
    try {
      if (kind === 'engine') {
        const res = await setEngineModel(sessionName, { model: alvo.id, effort: currentEffort ?? undefined });
        if (res.effort_error) {
          // O modelo pegou e o esforco nao: dizer "tudo certo" seria reportar sucesso sobre algo
          // que ficou pela metade.
          onApplied?.(res.model, null);
          err = m.modelo_trocado_esforco_nao({ erro: res.effort_error });
          aplicando = false;
          return;
        }
        onApplied?.(res.model, currentEffort ?? null);
      } else {
        // Nao se espera o picker: dispara, sai da frente e deixa a confirmacao com o usuario. Mas
        // a falha vai pra fora (`onFail`), nao pro console: o backend recusa a troca de verdade
        // (PickerError 409/422 quando o Claude nega ou o picker nao fecha), e engolir isso deixa
        // o pill mostrando um modelo que nunca entrou.
        Promise.resolve(onApply({ model: alvo.id, scope })).catch((e) =>
          onFail?.(e instanceof Error ? e.message : m.modelo_trocar_erro()),
        );
        atual = alvo.id;
        aplicando = false;
        onClose();
        return;
      }
      atual = alvo.id;
    } catch (e) {
      err = e instanceof Error ? e.message : m.comum_falha_aplicar();
      aplicando = false;
      return;
    }
    aplicando = false;
    onClose();
  }

</script>

<Popover {open} {anchor} {onClose} width={340} ariaLabel={m.modelo_titulo_claude()}>
  {#if buscavel}
    <div class="busca-wrap">
      <svg class="lupa" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor"
        stroke-width="2" stroke-linecap="round" aria-hidden="true">
        <circle cx="11" cy="11" r="7" /><path d="m20 20-3.5-3.5" />
      </svg>
      <input class="busca" type="search" data-foco bind:value={query}
        placeholder={m.comum_buscar_modelos()} aria-label={m.comum_buscar_modelo()} />
    </div>
  {/if}

  {#if err}
    <p class="err" role="alert">{err}</p>
  {/if}

  {#if loading && !models.length}
    <p class="vazio">{m.comum_carregando()}</p>
  {:else if !models.length}
    {#if !err}<p class="vazio">{m.comum_nenhum_modelo()}</p>{/if}
  {:else if !visiveis.length}
    <p class="vazio">{m.comum_nada_encontrado()}</p>
  {:else}
    <ul class="lista">
      <!-- Chave = id + nome, NAO so o id: o picker do Claude tem DUAS linhas com a keyword `opus`
           ("Opus" e "Opus (1M context)"), e chave repetida num `{#each}` com key derruba o render
           inteiro (each_key_duplicate) — a caixa ficava presa em "Carregando…", medido na tela. -->
      {#each visiveis as m (m.id + '|' + (m.name ?? ''))}
        <li>
          <button
            class="linha"
            class:ativa={escolhido === m.id}
            aria-pressed={escolhido === m.id}
            disabled={aplicando}
            data-foco={!buscavel && escolhido === m.id ? true : undefined}
            onclick={() => (escolhido = m.id)}
          >
            <span class="txt">
              <span class="nome">{rotulo(m)}</span>
              {#if detalhe(m)}<span class="det">{detalhe(m)}</span>{/if}
            </span>
            {#if escolhido === m.id}
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
    {#if escondidos}
      <p class="mais">{m.modelo_refine_busca({ n: escondidos })}</p>
    {/if}
    {#if onOpenPermission}
      <!-- Permissão mora aqui (saiu da fileira do composer — ver Props). Linha-irmã das de
           modelo: rótulo + modo atual, chevron avisa que abre outra caixa. -->
      <button class="linha perm-linha" onclick={onOpenPermission}>
        <span class="txt">
          <span class="nome">{m.composer_permissao()}</span>
          <span class="det">{permCurrent ?? '—'}</span>
        </span>
        <span class="chev" aria-hidden="true">›</span>
      </button>
    {/if}
    <div class="acoes">
      <button class="btn-aplicar" disabled={aplicando || !escolhido || escolhido === atual}
        onclick={() => aplicar('session')}>
        {aplicando ? m.modelo_aplicando() : m.modelo_aplicar_sessao()}
      </button>
      {#if kind === 'claude'}
        <button class="rodape" disabled={aplicando || !escolhido} onclick={() => aplicar('default')}>
          {m.modelo_salvar_padrao()}
        </button>
      {/if}
    </div>
  {/if}
</Popover>

<style>
  .busca-wrap {
    display: flex; align-items: center; gap: 6px;
    padding: 8px 10px; border-bottom: 1px solid var(--border-subtle);
  }
  .lupa { color: var(--text-muted); flex: none; }
  .busca {
    flex: 1; min-width: 0; background: transparent; border: none; outline: none;
    color: var(--text-primary); font-size: var(--text-sm);
  }
  .busca::placeholder { color: var(--text-muted); }

  .err { color: var(--error); font-size: var(--text-xs); margin: 8px 10px 0; }
  .vazio { color: var(--text-muted); font-size: var(--text-sm); text-align: center; padding: 14px 0; }

  .lista { list-style: none; margin: 0; padding: 4px 0; overflow-y: auto; }

  .linha {
    display: flex; align-items: center; gap: 8px; width: 100%;
    padding: 7px 10px; background: transparent; border: none;
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

  .txt { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 1px; }
  .nome { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .det {
    font-size: var(--text-xs); color: var(--text-muted);
    overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
  }
  .tick { flex: none; color: var(--accent); }

  .mais {
    font-size: var(--text-xs); color: var(--text-muted); text-align: center;
    padding: 6px 0; margin: 0; border-top: 1px solid var(--border-subtle);
  }

  /* Linha da permissão: separada da lista por uma hairline, com o chevron no lugar do tique. */
  .perm-linha { border-top: 1px solid var(--border-subtle); border-radius: 0; }
  .perm-linha .chev { flex: none; color: var(--text-muted); font-size: var(--text-base); }

  .acoes { border-top: 1px solid var(--border-subtle); padding: 8px; display: flex; flex-direction: column; gap: 4px; }

  .btn-aplicar {
    width: 100%; padding: 8px 10px; border: none; border-radius: var(--radius-md);
    background: var(--accent); color: var(--text-inverse);
    font-size: var(--text-sm); font-weight: 600; cursor: pointer;
  }
  .btn-aplicar:disabled { opacity: 0.5; cursor: default; }

  .rodape {
    width: 100%; padding: 8px 10px; background: transparent; border: none;
    color: var(--text-muted); font-size: var(--text-xs); text-align: center; cursor: pointer;
  }
  .rodape:hover:not(:disabled) { background: var(--bg-hover); color: var(--text-primary); }
</style>
