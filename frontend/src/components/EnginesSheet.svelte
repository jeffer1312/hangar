<script lang="ts">
  import BottomSheet from './BottomSheet.svelte';
  import ConfirmDialog from './ConfirmDialog.svelte';
  import {
    getEngines, getEnginesForServer,
    putEngine, putEngineForServer,
    deleteEngine, deleteEngineForServer,
    engineModelos, engineModelosForServer,
    type Motor, type ModeloProvedor,
  } from '../lib/api';
  import type { Server } from '../lib/auth';

  // Motores de modelo: rodar uma sessão em Kimi, num gateway próprio, ou em qualquer endpoint que
  // fale a Messages API — sem perder skills, hooks nem histórico, e sem tocar a conta Anthropic.
  //
  // Nada de catálogo chumbado: os modelos e a janela de contexto vêm do PRÓPRIO provedor
  // (GET /v1/models), com a chave do usuário. O valor muda por faixa de assinatura, então tabela
  // estática mentiria (o plano Moderato do Kimi dá 256k no k3, onde a doc fala de "até 1M").
  interface Props {
    open: boolean;
    onClose: () => void;
    targetServer?: Server | null;
  }
  let { open, onClose, targetServer = null }: Props = $props();

  const DICAS: { label: string; base_url: string }[] = [
    { label: 'Kimi Code', base_url: 'https://api.kimi.com/coding' },
    { label: 'OmniRoute (o seu)', base_url: 'https://ai.omniwise.com.br' },
  ];

  let motores = $state<Record<string, Motor>>({});
  let carregando = $state(false);
  let erro = $state('');
  let salvando = $state(false);
  // engines.json existe mas não pôde ser lido: bate em `motores: {}` igual a "nunca configurou
  // nada" (o backend não pode derrubar sessão/SSE por um hand-edit ruim), mas a tela TEM que dizer
  // a diferença — senão o usuário vê "nenhum motor ainda", re-adiciona um, e a próxima gravação
  // apaga os outros motores que estavam escondidos atrás do arquivo quebrado.
  let arquivoCorrompido = $state(false);
  let arquivoCaminho = $state('');

  // Defaults do bloco Avançado, espelhando engines.env_de: ligado = capacidade ativa. Os dois que
  // nascem LIGADOS (cache e raciocínio) são os que causam dano se desligados sem motivo — desligar
  // thinking rebaixa o modelo em alguns provedores.
  const PADRAO_AVANCADO = {
    bundled_skills: false,
    experimental_betas: false,
    prompt_caching: true,
    adaptive_thinking: true,
    tool_search: false,
    gateway_model_discovery: false,
    fine_grained_tool_streaming: false,
    auto_compact_window: '',
    max_output_tokens: '',
  };

  let form = $state<null | {
    nome: string;
    label: string;
    base_url: string;
    // Endereço como veio do disco, pra saber se o usuário mexeu nele (ver `enderecoMudou`).
    base_url_original: string;
    api_key: string;
    api_key_definida: boolean;
    model: string;
    subagent_model: string;
    context_window: string;
    // Avançado. Todos POSITIVOS ("marcado = ligado") — o backend traduz pras env vars DISABLE_*.
    bundled_skills: boolean;
    experimental_betas: boolean;
    prompt_caching: boolean;
    adaptive_thinking: boolean;
    tool_search: boolean;
    gateway_model_discovery: boolean;
    fine_grained_tool_streaming: boolean;
    auto_compact_window: string;
    max_output_tokens: string;
    existente: boolean;
  }>(null);

  let modelos = $state<ModeloProvedor[]>([]);
  let buscando = $state(false);
  let erroBusca = $state('');
  let okBusca = $state('');

  // Breakpoint desktop (mesmo corte do resto do app). Acima dele a sheet doca larga: o Avançado são
  // 9 controles e num painel de 365px eles viram corredor de texto.
  let isDesktop = $state(false);
  $effect(() => {
    const mq = window.matchMedia('(min-width: 820px)');
    const on = () => (isDesktop = mq.matches); on();
    mq.addEventListener('change', on);
    return () => mq.removeEventListener('change', on);
  });

  // Veredito específico do provedor. O formulário já conhece base_url/model, então dá pra dizer
  // "obrigatório AQUI" em vez de texto genérico — hoje só o caso documentado da Moonshot, onde
  // desligar o thinking rebaixa K3/K2.7 para K2.6 sem avisar. Não inventar regra pra provedor que
  // não tem doc: no resto, o veredito genérico continua valendo.
  const ehMoonshot = $derived(
    !!form && /moonshot|kimi/i.test(`${form.base_url} ${form.model}`),
  );

  // `/v1/models` respondeu no teste => o provedor expõe o endpoint que a descoberta consulta.
  // Sinal real medido no clique do usuário, melhor que "nem todo gateway expõe esse endpoint".
  const descobertaComprovada = $derived(modelos.length > 0);

  // Acordeão do "por quê?": um motivo aberto por vez (null = todos fechados).
  let porQue = $state<string | null>(null);

  // Acesso por chave pro snippet de linha: as 9 linhas viram 9 chamadas em vez de 9 blocos iguais.
  // Os tipos são a trava, não decoração — sem `as` em lugar nenhum, então errar a chave numa das 9
  // chamadas é erro de COMPILAÇÃO. Com cast, um typo passava no check e só aparecia rodando: o
  // toggle nascia sempre desligado e a escrita criava um campo fantasma que ia junto no salvar().
  type ChaveLiga = 'bundled_skills' | 'experimental_betas' | 'prompt_caching' | 'adaptive_thinking'
    | 'tool_search' | 'gateway_model_discovery' | 'fine_grained_tool_streaming';
  type ChaveNum = 'auto_compact_window' | 'max_output_tokens';
  // A própria chave decide o controle — não há parâmetro `tipo` pra discordar dela.
  const CHAVES_LIGA = ['bundled_skills', 'experimental_betas', 'prompt_caching', 'adaptive_thinking',
    'tool_search', 'gateway_model_discovery', 'fine_grained_tool_streaming'] as const;
  const ehLiga = (k: ChaveLiga | ChaveNum): k is ChaveLiga =>
    (CHAVES_LIGA as readonly string[]).includes(k);
  const ligado = (k: ChaveLiga) => !!form && form[k];
  const setLigado = (k: ChaveLiga, v: boolean) => { if (form) form[k] = v; };
  const numero = (k: ChaveNum) => (form ? form[k] : '');
  const setNumero = (k: ChaveNum, v: string) => { if (form) form[k] = v; };

  $effect(() => {
    if (!open) return;
    carregar();
  });

  async function carregar() {
    carregando = true;
    erro = '';
    try {
      const r = targetServer ? await getEnginesForServer(targetServer) : await getEngines();
      motores = r.motores;
      arquivoCorrompido = r.arquivo_corrompido;
      arquivoCaminho = r.arquivo_caminho;
    } catch (e) {
      erro = e instanceof Error ? e.message : 'Falha ao carregar';
    } finally {
      carregando = false;
    }
  }

  function novo() {
    form = {
      nome: '', label: '', base_url: '', base_url_original: '', api_key: '', api_key_definida: false,
      model: '', subagent_model: '', context_window: '',
      ...PADRAO_AVANCADO, existente: false,
    };
    modelos = []; erroBusca = ''; okBusca = ''; porQue = null;
  }

  function editar(nome: string) {
    const m = motores[nome];
    form = {
      nome,
      label: m.label ?? nome,
      base_url: m.base_url,
      base_url_original: m.base_url,
      // Nasce VAZIO de propósito: pré-preencher com a máscara faz qualquer toque mandar o texto
      // mascarado de volta e sobrescrever a chave real.
      api_key: '',
      api_key_definida: m.api_key_definida,
      model: m.model,
      subagent_model: m.subagent_model ?? '',
      context_window: m.context_window ? String(m.context_window) : '',
      // Espelha o default do backend: os que nascem LIGADOS saem só com `false` explícito
      // (`!== false`), os que nascem desligados exigem `=== true`.
      bundled_skills: m.bundled_skills === true,
      experimental_betas: m.experimental_betas === true,
      prompt_caching: m.prompt_caching !== false,
      adaptive_thinking: m.adaptive_thinking !== false,
      tool_search: m.tool_search === true,
      gateway_model_discovery: m.gateway_model_discovery === true,
      fine_grained_tool_streaming: m.fine_grained_tool_streaming === true,
      auto_compact_window: m.auto_compact_window ? String(m.auto_compact_window) : '',
      max_output_tokens: m.max_output_tokens ? String(m.max_output_tokens) : '',
      existente: true,
    };
    modelos = []; erroBusca = ''; okBusca = ''; porQue = null;
  }

  // Motor salvo, key vazia, mas o endereço no formulário já não é o que está no disco: testar
  // agora usaria a key salva contra o endereço ANTIGO (o servidor só aceita nome sozinho), o que
  // silenciosamente ignora a edição. Melhor avisar do que fingir que testou o novo endereço.
  const enderecoMudouSemChave = $derived(
    !!form && form.existente && !form.api_key.trim() && form.base_url.trim() !== form.base_url_original,
  );

  // Busca os modelos no provedor. Dobra de "Testar": 200 = chave boa, erro = mensagem do provedor.
  async function buscarModelos() {
    if (!form) return;
    buscando = true; erroBusca = ''; okBusca = '';
    try {
      // `nome` e `base_url`/`api_key` são mutuamente exclusivos no servidor (400 se vierem juntos)
      // — misturar mandaria a chave salva pra um endereço que o cliente digitou.
      const chave = form.api_key.trim();
      const corpo = chave
        ? { base_url: form.base_url.trim(), api_key: chave }
        : { nome: form.nome };
      const r = targetServer
        ? await engineModelosForServer(targetServer, corpo)
        : await engineModelos(corpo);
      modelos = r.modelos;
      okBusca = `${r.modelos.length} modelo(s) — conexão e chave OK`;
      const atual = modelos.find((m) => m.id === form!.model) ?? modelos[0];
      if (atual) escolherModelo(atual.id);
    } catch (e) {
      erroBusca = e instanceof Error ? e.message : 'Falha ao consultar o provedor';
    } finally {
      buscando = false;
    }
  }

  function escolherModelo(id: string) {
    if (!form) return;
    form.model = id;
    const m = modelos.find((x) => x.id === id);
    // A janela vem do provedor: errar aqui custa capacidade real (em branco o Claude Code assume
    // 200k e compacta cedo, mesmo num modelo de 500k). Modelo sem context_length limpa o campo —
    // deixar o número do modelo ANTERIOR salvo faria a var passar da janela real do modelo novo, e
    // o provedor erra no meio do turno em vez do Claude Code compactar.
    form.context_window = m?.context_length ? String(m.context_length) : '';
  }

  const modeloAtual = $derived(form ? modelos.find((m) => m.id === form!.model) : undefined);

  async function salvar() {
    if (!form) return;
    salvando = true; erro = '';
    try {
      const corpo: Record<string, unknown> = {
        label: form.label.trim() || form.nome.trim(),
        base_url: form.base_url.trim(),
        model: form.model.trim(),
      };
      if (form.api_key.trim()) corpo.api_key = form.api_key.trim();
      if (form.subagent_model.trim()) corpo.subagent_model = form.subagent_model.trim();
      if (form.context_window) corpo.context_window = Number(form.context_window);

      // O PUT é substituição TOTAL do registro (backend/app/engines.py salvar()/_normalizar()):
      // omitir um campo aqui não é "deixar como estava", é apagá-lo no disco. `modeloAtual` só
      // existe depois de um Testar NESTA sessão do form — editar só o rótulo/janela e salvar sem
      // retestar não pode perder vision/tool_search calado. Precedência: valor recém-testado >
      // valor já salvo pro motor > omitir (motor novo, nunca testado).
      const salvo = motores[form.nome];
      const vision = typeof modeloAtual?.vision === 'boolean' ? modeloAtual.vision : salvo?.vision;
      if (typeof vision === 'boolean') corpo.vision = vision;
      // Avançado: a UI seta TODOS, então vão sempre no corpo — não dependem de preservar o disco.
      corpo.bundled_skills = form.bundled_skills;
      corpo.experimental_betas = form.experimental_betas;
      corpo.prompt_caching = form.prompt_caching;
      corpo.adaptive_thinking = form.adaptive_thinking;
      corpo.tool_search = form.tool_search;
      corpo.gateway_model_discovery = form.gateway_model_discovery;
      corpo.fine_grained_tool_streaming = form.fine_grained_tool_streaming;
      if (form.auto_compact_window) corpo.auto_compact_window = Number(form.auto_compact_window);
      if (form.max_output_tokens) corpo.max_output_tokens = Number(form.max_output_tokens);

      motores = targetServer
        ? (await putEngineForServer(targetServer, form.nome.trim(), corpo)).motores
        : (await putEngine(form.nome.trim(), corpo)).motores;
      form = null;
    } catch (e) {
      erro = e instanceof Error ? e.message : 'Falha ao salvar';
    } finally {
      salvando = false;
    }
  }

  // Confirmação de remoção: sheet temática, não o confirm() nativo do navegador (quebra o tema
  // escuro). ConfirmDialog (não ConfirmSheet) de propósito — EnginesSheet já É um BottomSheet
  // aberto, e ConfirmSheet também embrulha um BottomSheet, cujo <svelte:window onkeydown> de Escape
  // registraria um SEGUNDO handler global disputando com o desta sheet (a mesma race de foco que
  // motivou tirar o EnginesSheet de dentro do ConfigSheet). ConfirmDialog trata Escape só no próprio
  // elemento do diálogo — é o padrão que o LoopSheet já usa pro mesmo caso (confirm dentro de sheet
  // aberta).
  let confirmRemoverNome = $state<string | null>(null);

  function remover(nome: string) {
    confirmRemoverNome = nome;
  }

  async function removerConfirmado() {
    const nome = confirmRemoverNome;
    confirmRemoverNome = null;
    if (!nome) return;
    erro = '';
    try {
      if (targetServer) await deleteEngineForServer(targetServer, nome);
      else await deleteEngine(nome);
      const { [nome]: _fora, ...resto } = motores;
      motores = resto;
    } catch (e) {
      erro = e instanceof Error ? e.message : 'Falha ao remover';
    }
  }
</script>

<BottomSheet {open} {onClose} ariaLabel="Motores de modelo" wide={isDesktop} centered={isDesktop}>
  <div class="mot">
    <header class="mot-head">
      <h2>Motores de modelo</h2>
      <p class="sub">
        Rode uma sessão em outro modelo sem perder skills, hooks nem histórico. Sua conta Anthropic
        fica intocada — o motor vale só para a sessão que você abrir com ele.
      </p>
    </header>

    {#if carregando}
      <p class="aviso">Carregando…</p>
    {:else if form}
      <div class="form">
        <label class="campo">
          <span class="rot">Nome curto</span>
          <input type="text" placeholder="kimi" autocapitalize="off" spellcheck={false}
                 disabled={form.existente}
                 value={form.nome} oninput={(e) => (form!.nome = e.currentTarget.value)} />
          <span class="ajuda">
            No terminal você usa <code>claude-engine {form.nome || 'kimi'}</code>. Minúsculas, sem espaço.
          </span>
        </label>

        <label class="campo">
          <span class="rot">Endereço da API</span>
          <input type="text" autocapitalize="off" spellcheck={false} placeholder="https://…"
                 value={form.base_url} oninput={(e) => (form!.base_url = e.currentTarget.value)} />
          <span class="ajuda">
            <strong>Sem o /v1 no fim</strong> — o Claude Code monta o caminho sozinho. Precisa falar a
            Messages API da Anthropic; gateway só-OpenAI exige um proxy tradutor em 127.0.0.1.
            Endereço público tem que ser https, senão a chave viaja em claro.
          </span>
          <span class="dicas">
            {#each DICAS as d (d.base_url)}
              <button class="dica" onclick={() => (form!.base_url = d.base_url)}>{d.label}</button>
            {/each}
          </span>
        </label>

        <label class="campo">
          <span class="rot">Chave da API</span>
          {#if form.api_key_definida}
            <span class="def">chave configurada — deixe vazio para manter</span>
          {/if}
          <input type="text" autocomplete="off" autocapitalize="off" spellcheck={false}
                 placeholder={form.api_key_definida ? 'colar nova chave para trocar' : 'colar a chave'}
                 value={form.api_key} oninput={(e) => (form!.api_key = e.currentTarget.value)} />
        </label>

        <div class="campo">
          <button class="btn" onclick={buscarModelos}
                  disabled={buscando || !form.base_url.trim() || (!form.api_key.trim() && !form.api_key_definida)}>
            {buscando ? 'Consultando…' : 'Testar e listar modelos'}
          </button>
          {#if okBusca}<span class="ok">{okBusca}</span>{/if}
          {#if enderecoMudouSemChave}
            <!-- Testar sem chave nova reusa a key salva, mas o servidor só aceita isso com o
                 endereço TAMBÉM salvo — o endereço que você acabou de editar aqui não entra no
                 teste até você colar a chave de novo. -->
            <span class="ajuda erro">Você mudou o endereço: para testar um endereço novo, cole a chave também.</span>
          {/if}
          <!-- A mensagem crua do provedor é a informação útil ("401 Invalid Authentication"). -->
          {#if erroBusca}<span class="ajuda erro">{erroBusca}</span>{/if}
        </div>

        <label class="campo">
          <span class="rot">Modelo</span>
          {#if modelos.length}
            <select value={form.model} onchange={(e) => escolherModelo(e.currentTarget.value)}>
              {#each modelos as m (m.id)}
                <option value={m.id}>
                  {m.id}{m.context_length ? ` · ${Math.round(m.context_length / 1000)}k` : ''}
                </option>
              {/each}
            </select>
          {:else}
            <input type="text" placeholder="id do modelo" autocapitalize="off" spellcheck={false}
                   value={form.model} oninput={(e) => (form!.model = e.currentTarget.value)} />
            <span class="ajuda">Use "Testar" acima para listar os ids do seu provedor.</span>
          {/if}
          {#if modeloAtual?.vision === false}
            <!-- O app manda foto do celular; motor cego quebra esse fluxo e o usuário tem que saber
                 ANTES de abrir a sessão, não quando a foto for ignorada. -->
            <span class="ajuda erro">Este modelo não enxerga imagem: anexar foto não vai funcionar.</span>
          {/if}
        </label>

        <label class="campo">
          <span class="rot">Modelo dos subagentes</span>
          {#if modelos.length}
            <select value={form.subagent_model}
                    onchange={(e) => (form!.subagent_model = e.currentTarget.value)}>
              <option value="">mesmo que o principal</option>
              {#each modelos as m (m.id)}
                <option value={m.id}>{m.id}</option>
              {/each}
            </select>
          {:else}
            <input type="text" placeholder="vazio = mesmo que o principal" autocapitalize="off" spellcheck={false}
                   value={form.subagent_model} oninput={(e) => (form!.subagent_model = e.currentTarget.value)} />
          {/if}
          <span class="ajuda">
            Subagentes fazem busca mecânica repetitiva; um modelo mais barato aqui é economia real,
            sem tocar no modelo principal da sessão.
          </span>
        </label>

        <label class="campo">
          <span class="rot">Janela de contexto</span>
          <input type="number" inputmode="numeric" min="1" placeholder="tokens"
                 value={form.context_window}
                 oninput={(e) => (form!.context_window = e.currentTarget.value)} />
          <span class="ajuda">
            Vem do provedor ao testar. Em branco, o Claude Code assume 200k e compacta cedo mesmo num
            modelo maior — capacidade jogada fora. Confira na sessão com <code>/context</code>.
          </span>
        </label>

        <details class="avancado">
          <summary>Avançado — recursos do harness</summary>
          <p class="ajuda topo">
            O Claude Code manda recursos que só a API da Anthropic entende; num provedor de terceiro
            eles são recusados ou ignorados. Cada linha já traz o veredito: abra "por quê?" só se
            quiser o motivo.
          </p>

          <!-- Uma linha por recurso, no MESMO vocabulário do ConfigSheet (rótulo à esquerda,
               controle à direita, separador entre linhas). O motivo é um acordeão: só um aberto por
               vez, senão nove parágrafos abertos viram de novo a parede de texto que isto resolve. -->
          {#snippet linha(chave: ChaveLiga | ChaveNum, rot: string, vered: string, tom: string,
                          motivo: import('svelte').Snippet, morto = false)}
            <div class="linha" class:morta={morto}>
              <div class="txt">
                <span class="rot">{rot}</span>
                <span class="meta">
                  <span class="vered {tom}">{vered}</span>
                  <button type="button" class="pq" aria-expanded={porQue === chave}
                          onclick={() => (porQue = porQue === chave ? null : chave)}>
                    por quê?<span class="chev" class:aberta={porQue === chave} aria-hidden="true">▾</span>
                  </button>
                </span>
              </div>
              {#if ehLiga(chave)}
                <input class="switch" type="checkbox" disabled={morto}
                       checked={ligado(chave)} aria-label={rot}
                       onchange={(e) => setLigado(chave, e.currentTarget.checked)} />
              {:else}
                <input class="num" type="number" inputmode="numeric" min="1" placeholder="padrão"
                       aria-label={rot} value={numero(chave)}
                       oninput={(e) => setNumero(chave, e.currentTarget.value)} />
              {/if}
            </div>
            {#if porQue === chave}
              <p class="motivo">{@render motivo()}</p>
            {/if}
          {/snippet}

          {#snippet mSkills()}
            Uma invocação da <code>claude-api</code> injeta 206k tokens de uma vez (medido: 12% → 67%
            da janela num modelo de 372k). Suas skills de plugin e de <code>~/.claude/skills</code>
            não são afetadas.
          {/snippet}
          {#snippet mBetas()}
            Campos beta que o Claude Code envia sozinho (<code>context_management</code> e os campos
            beta de tool). Ligado num upstream que não os aceita dá
            <code>400 Extra inputs are not permitted</code>.
          {/snippet}
          {#snippet mCache()}
            Num gateway o cache já degrada sozinho e calado: se o breakpoint for recusado, aquele
            bloco fica sem cache pelo resto da conversa, sem erro. Desligar só faz sentido em
            provedor que cobra o cache mais caro que o miss.
          {/snippet}
          {#snippet mThinking()}
            Desligar é destrutivo em alguns provedores: a doc da Moonshot diz que sem thinking o K3 e
            o K2.7 caem para K2.6, calado.
            {#if ehMoonshot}Este endereço é Moonshot/Kimi, então a regra vale aqui.{/if}
            Só desmarque se o upstream devolver <code>400</code> citando <code>thinking</code> ou
            <code>adaptive</code>.
          {/snippet}
          {#snippet mToolSearch()}
            {#if !form?.experimental_betas}
              O desligamento dos betas mantém o tool search off e este controle não sobrepõe. Ligue
              os betas antes, se o provedor os aceitar.
            {:else}
              Carrega as definições de tool sob demanda em vez de todas por turno (50 tools ≈ 10–20k
              tokens). O Claude Code já desliga sozinho quando a URL não é da Anthropic, porque a
              maioria dos proxies não repassa os blocos <code>tool_reference</code>.
            {/if}
          {/snippet}
          {#snippet mDescoberta()}
            Consulta o <code>/v1/models</code> do provedor e popula o seletor <code>/model</code>
            dentro da sessão. É o mesmo endpoint que o botão "Testar" aqui em cima chama: se ele
            trouxe modelos, a descoberta funciona.
          {/snippet}
          {#snippet mStreaming()}
            O Claude Code desliga por padrão atrás de URL customizada. Ligue se o provedor suportar
            streaming parcial dos argumentos de tool.
          {/snippet}
          {#snippet mCompactar()}
            Preencha quando o provedor impõe uma janela menor que a do modelo e reescreve a mensagem
            de erro: o Claude Code não reconhece, não compacta sozinho, e a sessão trava em
            <code>exceeds the context window</code>. Deixe abaixo do limite real do provedor.
          {/snippet}
          {#snippet mSaida()}
            Par do campo acima. Mantenha abaixo do limite de saída do modelo no provedor.
          {/snippet}

          <div class="grade">
            {@render linha('bundled_skills', 'Skills empacotadas', 'Recomendado: desligado', '', mSkills)}
            {@render linha('experimental_betas', 'Recursos beta', 'Recomendado: desligado', '', mBetas)}
            {@render linha('prompt_caching', 'Prompt caching', 'Recomendado: ligado', 'sim', mCache)}
            {@render linha('adaptive_thinking', 'Raciocínio adaptativo',
              ehMoonshot ? 'Obrigatório neste provedor' : 'Recomendado: ligado',
              ehMoonshot ? 'forte' : 'sim', mThinking)}
            {@render linha('tool_search', 'Tool search',
              form.experimental_betas ? 'Recomendado: desligado' : 'Sem efeito: betas desligados',
              '', mToolSearch, !form.experimental_betas)}
            {@render linha('gateway_model_discovery', 'Descoberta de modelos',
              descobertaComprovada ? `Recomendado: ligado (${modelos.length} modelos)` : 'Teste o provedor antes',
              descobertaComprovada ? 'sim' : '', mDescoberta)}
            {@render linha('fine_grained_tool_streaming', 'Streaming fino de tool', 'Recomendado: desligado', '', mStreaming)}
            {@render linha('auto_compact_window', 'Disparar compactação em', 'Recomendado: em branco', '', mCompactar)}
            {@render linha('max_output_tokens', 'Teto de saída', 'Recomendado: em branco', '', mSaida)}
          </div>
        </details>

        {#if erro}<p class="aviso erro">{erro}</p>{/if}

        <div class="acoes">
          <button class="btn" onclick={() => (form = null)}>Cancelar</button>
          <button class="btn primario" onclick={salvar}
                  disabled={salvando || !form.nome.trim() || !form.model.trim() || !form.base_url.trim()}>
            {salvando ? 'Salvando…' : 'Salvar'}
          </button>
        </div>
      </div>
    {:else}
      {#if arquivoCorrompido}
        <!-- Não é "nenhum motor configurado": o arquivo existe mas está com erro de formato, e
             pode estar escondendo motores reais (com as keys deles) atrás do erro de leitura.
             Adicionar um motor agora falha ao salvar de propósito (o backend recusa sobrescrever
             um arquivo que não conseguiu ler) — melhor isso do que apagar o que já está lá. -->
        <p class="aviso erro">
          Não consegui ler <code>{arquivoCaminho}</code> — o arquivo existe mas está com erro de
          formato (JSON inválido). Se você já tinha motores configurados, eles continuam no
          arquivo: corrija-o à mão (ou restaure um backup) antes de adicionar um novo.
        </p>
      {:else if !Object.keys(motores).length}
        <p class="aviso">
          Nenhum motor ainda. Adicione um e ele aparece no seletor ao criar sessão, e no terminal como
          <code>claude-engine &lt;nome&gt;</code>.
        </p>
      {/if}
      <div class="lista">
        {#each Object.entries(motores) as [nome, m] (nome)}
          <div class="card">
            <div class="card-txt">
              <span class="card-nome">{m.label ?? nome}</span>
              <span class="card-sub">{m.model}{m.context_window ? ` · ${Math.round(m.context_window / 1000)}k` : ''}</span>
              <span class="card-url">{m.base_url}</span>
              <span class="card-key">{m.api_key}</span>
            </div>
            <div class="card-acoes">
              <button class="btn" onclick={() => editar(nome)}>Editar</button>
              <button class="btn perigo" onclick={() => remover(nome)}>Remover</button>
            </div>
          </div>
        {/each}
      </div>
      {#if erro}<p class="aviso erro">{erro}</p>{/if}
      <button class="btn primario largo" onclick={novo}>Adicionar motor</button>
    {/if}
  </div>
</BottomSheet>

{#if confirmRemoverNome}
  <ConfirmDialog title={`Remover o motor "${confirmRemoverNome}"?`} aria="Remover motor"
    onClose={() => (confirmRemoverNome = null)}
    actions={[
      { label: 'Cancelar', onClick: () => (confirmRemoverNome = null) },
      { label: 'Remover', kind: 'danger', onClick: removerConfirmado },
    ]}>
    <p class="ajuda">Sessões abertas nele continuam rodando.</p>
  </ConfirmDialog>
{/if}

<style>
  .mot { padding: var(--space-2) var(--space-4) var(--space-4); }
  .mot-head h2 { margin: 0; font-size: var(--text-lg); font-weight: 600; color: var(--text-primary); }
  .mot-head .sub { margin: 2px 0 var(--space-4); font-size: var(--text-xs); color: var(--text-muted); line-height: 1.45; }

  .lista { display: flex; flex-direction: column; gap: var(--space-3); }
  .card {
    display: flex; align-items: flex-start; justify-content: space-between; gap: var(--space-3);
    padding: var(--space-3);
    background: var(--bg-elevated);
    border: 1px solid var(--border-subtle);
    border-radius: var(--radius-md);
  }
  .card-txt { display: flex; flex-direction: column; gap: 2px; min-width: 0; }
  .card-nome { font-size: var(--text-base); font-weight: 600; color: var(--text-primary); }
  .card-sub { font-family: var(--font-mono); font-size: var(--text-xs); color: var(--text-secondary); }
  .card-url, .card-key {
    font-family: var(--font-mono); font-size: 11px; color: var(--text-muted);
    overflow: hidden; text-overflow: ellipsis; white-space: nowrap; max-width: 100%;
  }
  .card-acoes { display: flex; flex-direction: column; gap: var(--space-2); flex-shrink: 0; }

  .form { display: flex; flex-direction: column; gap: var(--space-4); }
  .campo { display: flex; flex-direction: column; gap: var(--space-2); }
  .rot { font-size: var(--text-base); font-weight: 600; color: var(--text-primary); }
  .ajuda { font-size: var(--text-xs); color: var(--text-muted); line-height: 1.45; }
  .ajuda.erro { color: var(--error); }

  /* Avançado: recolhido por padrão — são 9 controles que a maioria nunca toca. <details> nativo em
     vez de estado próprio; o toggle já vem acessível e some com prefers-reduced-motion sem regra. */
  .avancado {
    border: 1px solid var(--border-subtle); border-radius: var(--radius-md);
    padding: var(--space-3) var(--space-4);
    display: flex; flex-direction: column; gap: var(--space-3);
  }
  .avancado > summary {
    cursor: pointer; font-size: var(--text-sm); font-weight: 600; color: var(--text-secondary);
    /* Sem gap quando fechado: o `gap` do flex vale só entre filhos visíveis, e o summary é o único. */
    margin: calc(var(--space-3) * -1) 0;
  }
  .avancado[open] > summary { margin-bottom: 0; }
  .avancado .ajuda.topo { margin: 0; max-width: 68ch; }

  /* Linha de recurso: MESMO vocabulário do ConfigSheet (rótulo à esquerda, controle à direita,
     separador entre linhas). Nada de card por item — nove cards viram ruído, e o separador já
     agrupa. Grid de 3 faixas pra o motivo expandido nascer alinhado sob o texto, não sob o
     controle. */
  .grade { display: grid; grid-template-columns: 1fr; }
  .linha {
    display: flex; align-items: center; justify-content: space-between; gap: var(--space-4);
    padding: var(--space-3) 0;
    border-top: 1px solid var(--border-subtle);
  }
  .linha:first-of-type { border-top: none; }
  /* Recurso inerte por dependência (tool search sem os betas): esmaece, mas o veredito continua
     legível — ele é justamente quem explica por que o controle está morto. */
  .linha.morta .rot { color: var(--text-muted); }

  .txt { display: flex; flex-direction: column; gap: 2px; min-width: 0; }
  .avancado .rot { font-size: var(--text-sm); font-weight: 600; color: var(--text-primary); }
  .meta { display: flex; align-items: center; flex-wrap: wrap; gap: var(--space-2); font-size: 11px; }
  .vered { color: var(--text-muted); }
  .vered.sim { color: var(--success); }
  .vered.forte { color: var(--warning); font-weight: 600; }

  /* "por quê?" é um botão de verdade, não texto solto: cor de link, sublinhado no hover e uma seta
     que gira. Sem afordância ninguém percebe que abre. */
  .pq {
    display: inline-flex; align-items: center; gap: 3px;
    padding: 4px 0; background: none; border: none; cursor: pointer;
    font-size: 11px; color: var(--accent);
  }
  .pq:hover { text-decoration: underline; }
  .pq .chev { font-size: 8px; transition: transform 160ms var(--ease-out); }
  .pq .chev.aberta { transform: rotate(180deg); }
  .motivo {
    margin: 0 0 var(--space-3);
    font-size: var(--text-xs); color: var(--text-secondary); line-height: 1.5; max-width: 62ch;
  }

  /* `.switch` é global (app.css) — vocabulário único de liga/desliga do app. */
  .avancado input.num { width: 132px; flex-shrink: 0; }

  /* Desktop: o modal é largo, então as 9 linhas viram duas colunas de lista. `column` (multicol)
     em vez de grid porque o motivo expandido tem altura variável — no grid ele abriria um buraco
     na coluna vizinha, que foi exatamente o que ficou feio na primeira versão. */
  @media (min-width: 820px) {
    .grade { display: block; columns: 2; column-gap: var(--space-7); }
    /* break-inside evita a linha ser cortada ao meio na virada de coluna. */
    .linha, .motivo { break-inside: avoid; }
    /* Na multicol, :first-of-type só acerta a primeira do documento; a primeira da 2a coluna
       ficaria com um separador solto no topo. Borda embaixo resolve nas duas colunas. */
    .linha { border-top: none; border-bottom: 1px solid var(--border-subtle); }
  }

  .def { font-size: 11px; color: var(--success); }
  .ok { font-size: var(--text-xs); color: var(--success); }
  .dicas { display: flex; flex-wrap: wrap; gap: var(--space-2); }
  .dica {
    background: var(--bg-elevated); color: var(--text-secondary);
    border: 1px solid var(--border-subtle); border-radius: var(--radius-full);
    padding: 2px 10px; font-size: 11px;
  }

  input[type='text'], input[type='number'], select {
    height: 40px;
    background: var(--bg-base);
    border: 1px solid var(--border-default);
    border-radius: var(--radius-sm);
    color: var(--text-primary);
    font-family: var(--font-mono);
    font-size: 16px;                 /* 16px evita o zoom automático do iOS ao focar */
    padding: 0 var(--space-3);
    outline: none;
    min-width: 0;
  }
  input:focus, select:focus { border-color: var(--accent); }
  input:disabled { opacity: 0.6; }

  /* Ações grudam no rodapé: o formulário de motor é alto (com o Avançado aberto passa de duas telas)
     e Salvar/Cancelar sumiam lá embaixo. Fundo sólido + borda pra o conteúdo não passar por baixo. */
  .acoes {
    display: flex; justify-content: flex-end; gap: var(--space-3);
    position: sticky; bottom: calc(env(safe-area-inset-bottom) * -1 - var(--space-5));
    margin: 0 calc(var(--space-4) * -1) calc(var(--space-4) * -1);
    padding: var(--space-3) var(--space-4);
    padding-bottom: calc(env(safe-area-inset-bottom) + var(--space-3));
    background: var(--bg-elevated);
    border-top: 1px solid var(--border-subtle);
  }
  .aviso { font-size: var(--text-sm); color: var(--text-muted); margin: var(--space-3) 0; }
  .aviso.erro { color: var(--error); }
  .btn {
    height: 40px; padding: 0 var(--space-4);
    border-radius: var(--radius-md);
    background: var(--bg-elevated); color: var(--text-primary);
    font-size: var(--text-sm); font-weight: 600;
  }
  .btn.primario { background: var(--accent); color: #fff; }
  .btn.perigo { color: var(--error); }
  .btn.largo { width: 100%; margin-top: var(--space-4); }
  .btn:disabled { opacity: 0.45; }
</style>
