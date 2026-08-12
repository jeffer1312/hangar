<script lang="ts">
  import { untrack } from 'svelte';
  import BottomSheet from './BottomSheet.svelte';
  import Select from './Select.svelte';
  import FolderScanner from './FolderScanner.svelte';
  import { getSessions, listClaudeConfigs, getEngines, criarConta, apagarConta,
           modelOptions, type ModelOption, type Motor } from '../lib/api';
  import { basename, providerName } from '../lib/format';
  import { selectServer, getActiveId, serverColor } from '../lib/auth';
  import type { Server } from '../lib/auth';
  import type { SessionInfo, ConfigDirInfo, Provider } from '../lib/types';

  interface Props {
    open: boolean;
    servers: Server[];
    onClose: () => void;
    onCreate: (name: string, cwd?: string, configDir?: string | null, provider?: Provider,
               engine?: string | null, model?: string | null, effort?: string | null) => Promise<void>;
    onOpenSession: (name: string) => void;
  }
  let { open, servers, onClose, onCreate, onOpenSession }: Props = $props();

  // Provider da sessao nova: Claude (padrao, tmux), Codex (app-server, sem tmux/config_dir), Pi ou
  // Kimi (pane tmux como o Claude, mas sem config_dir e sem motor — o backend recusa motor fora do
  // Claude com 400, entao os dois pickers abaixo seguem Claude-only).
  const PROVIDERS: Provider[] = ['claude', 'codex', 'pi', 'kimi'];
  let provider = $state<Provider>('claude');

  // Servidor-alvo da nova sessão. Como o scanner/dedupe/criação leem o servidor ATIVO, escolher
  // aqui = selectServer(id): todas as chamadas seguintes do sheet caem nesse backend.
  let targetServer = $state(getActiveId() ?? '');
  function pickTarget(id: string) {
    targetServer = id;
    selectServer(id);
    // Configs sao DO SERVIDOR DE DESTINO: re-busca a cada troca (a pasta ja re-busca via {#key};
    // sem isto a lista ficava a do servidor da ABERTURA e o create mandava um config_dir de outra
    // maquina -> 400 "config_dir invalido" no backend de destino).
    loadConfigs();
  }

  // Fluxo em dois passos: 1) escolher a pasta (scanner) -> 2) criar uma sessao nova com nome UNICO
  // derivado do basename. Varias sessoes na mesma pasta sao permitidas (cada uma tem nome+jsonl
  // proprio); reabrir uma existente = clicar no card da lista.
  let picked = $state<string | null>(null);
  let name = $state('');
  let checking = $state(false);
  let takenNames = $state<Set<string>>(new Set());
  let hasSameFolder = $state(false);
  let loading = $state(false);
  let error = $state('');

  // Nome unico p/ tmux: sanitiza (igual ao backend) e, se ja existir, sufixa -2/-3...
  function uniqueName(base: string, taken: Set<string>): string {
    const clean = base.replace(/[^A-Za-z0-9_-]/g, '-').replace(/^-+|-+$/g, '') || 'sessao';
    if (!taken.has(clean)) return clean;
    let i = 2;
    while (taken.has(`${clean}-${i}`)) i++;
    return `${clean}-${i}`;
  }

  // Config dirs do Claude (ex: ~/.claude, ~/.claude-work). Picker so aparece quando ha mais de um.
  let configs = $state<ConfigDirInfo[]>([]);
  let selectedConfig = $state<string | null>(null);
  // Motor de modelo (Task 5): '' = conta Anthropic (o padrão de sempre). Só faz sentido com provider claude.
  let engine = $state('');
  let motores = $state<Record<string, Motor>>({});

  // Escolha de modelo e esforço (só claude e pi — ver NIVEIS abaixo). `''` = Padrão, ou seja
  // nenhuma flag no comando: comportamento de hoje, byte por byte. A lista vem do
  // GET /api/model-options (Task 4), que já traz contexto/👁 pros provedores que informam.
  let modelo = $state('');
  let esforco = $state('');
  let modelos = $state<ModelOption[]>([]);
  let listaReduzida = $state(false);
  let erroModelos = $state('');

  // Listas FECHADAS por provider, as mesmas do backend (model_args.py). O Kimi é o QUARTO botão da
  // tela e não tem nível de esforço nenhum: uma guarda `provider !== 'codex'` deixaria ele entrar
  // aqui e `NIVEIS['kimi']` seria undefined — o `.map` derrubaria a folha inteira. Por isso a
  // lista explícita dos dois providers em escopo.
  const NIVEIS: Record<string, string[]> = {
    claude: ['low', 'medium', 'high', 'xhigh', 'max'],
    pi: ['off', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max'],
  };

  // `targetServer` (acima) é o servidor de destino. Ele entra na chave porque MOTOR É POR SERVIDOR
  // (comentário do loadConfigs): sem isso o app lembraria um modelo de motor que o outro servidor
  // não tem — a sessão subiria com --model de um id que aquele provedor não conhece.
  const chaveMemoria = () => `cp_last_model:${targetServer}:${provider}:${engine || '-'}`;

  // Mesma guarda de sequência que o sheet já usa pra configs/motores (`cfgSeq`), pelo mesmo motivo
  // escrito lá: provider → motor → config em sequência rápida deixa várias respostas em voo, e a
  // mais LENTA venceria — o usuário escolheria um modelo que não existe no que acabou de selecionar.
  let modSeq = 0;

  // Catálogo do Pi traz ids que se REPETEM entre providers (medido: k3 existe na kimi-coding E na
  // kimi-jefferson; gpt-5.6-luna na openai-codex E na opencode-go). O valor da opção tem que ser
  // provider/id: sem o provider, a chave do each do Select colide e o each_key_duplicate derruba a
  // lista inteira — e o `pi --model k3` cru seria ambíguo e recusado pelo CLI (o model-resolver do
  // Pi rejeita bare id que casa em mais de um provider). Nos outros formatos (motor, cache do
  // Claude, aliases reduzidos) não há provider e o id já é único — o valor é o id puro, como hoje.
  const valorModelo = (m: ModelOption) => (m.provider ? `${m.provider}/${m.id}` : m.id);

  async function carregarModelos() {
    const seq = ++modSeq;
    modelos = []; erroModelos = ''; listaReduzida = false; modelo = ''; esforco = '';
    // Só os dois providers em escopo. `provider !== 'codex'` NÃO serve: a tela tem quatro botões
    // (claude, codex, pi, kimi) e o Kimi não tem nível de esforço nenhum.
    if (provider !== 'claude' && provider !== 'pi') return;
    try {
      const r = await modelOptions(provider, engine, selectedConfig);
      if (seq !== modSeq) return;
      modelos = r.models;
      listaReduzida = r.reduced;
      const lembrado = localStorage.getItem(chaveMemoria());
      // Casar contra a lista: modelo tirado do provedor ou motor removido não pode virar flag às
      // cegas — a sessão subiria e falharia no primeiro turno.
      if (lembrado && modelos.some((m) => valorModelo(m) === lembrado)) modelo = lembrado;
      esforco = localStorage.getItem(chaveMemoria() + ':effort') ?? '';
    } catch (e) {
      if (seq !== modSeq) return;
      erroModelos = e instanceof Error ? e.message : 'não consegui listar os modelos';
    }
  }
  // Criar conta pela tela: botão "+ conta" no seletor. Ocupada = POST em voo (desabilita o botão);
  // avisoConta leva a mensagem pro usuário (sucesso ou erro).
  let contaOcupada = $state(false);
  let avisoConta = $state('');
  // Path da conta criada com sucesso nesta abertura: o aviso do /login só vale enquanto a
  // seleção aponta pra ela — trocou a conta, o texto não pode continuar afirmando o que a
  // próxima sessão não vai ser. Erro e "criada mas não listada" ficam com null.
  let contaCriadaPath = $state<string | null>(null);
  // O aviso é erro (role=alert) ou status (role=status) — leitor de tela anuncia diferente.
  let contaErro = $state(false);
  // Guard de corrida: trocas rapidas de servidor deixam fetches em voo; so a resposta do ULTIMO
  // pedido pode escrever (senao a lista de um servidor antigo aterrissava por cima da atual).
  // Motores tambem sao POR SERVIDOR (cada um tem seu proprio ~/.claude/engines.json) -- por isso
  // entram no MESMO seq/guard que os configs, e nao num effect a parte: sem isto, trocar o
  // servidor-alvo depois de abrir o sheet recarregava configs mas deixava a lista de motores do
  // servidor ANTERIOR na tela (create() ia pro servidor novo com um motor que pode nem existir la).
  let cfgSeq = 0;
  function loadConfigs() {
    const seq = ++cfgSeq;
    configs = [];
    selectedConfig = null;
    motores = {};
    // Zera o estado da linha de conta AQUI porque este `++cfgSeq` invalida qualquer criar/apagar em
    // voo: o `finally` daquela operação só libera o botão se a geração dele ainda for a vigente, e
    // agora não é. Sem este reset o "+ conta" ficava em "…" e `aria-busy` pra sempre, e só fechar e
    // reabrir o sheet destravava. A conta EM VOO pode ter sido criada no servidor de origem — o
    // aviso dela se perde junto, e é por isso que os chips e o seletor ficam desabilitados enquanto
    // `contaOcupada` (ver o template): a troca de servidor no meio da operação deixa de acontecer.
    contaOcupada = false;
    avisoConta = '';
    contaErro = false;
    contaCriadaPath = null;
    pedindoNome = false;
    confirmandoApagar = false;
    listClaudeConfigs()
      .then((cs) => {
        if (seq !== cfgSeq) return;
        configs = cs;
        selectedConfig = cs.find((c) => c.active)?.path ?? cs[0]?.path ?? null;
        // DENTRO do .then, nunca depois de loadConfigs(): ela não é async — dispara os fetches e
        // volta na hora, e `selectedConfig` só é preenchido aqui. Chamar logo depois mandaria
        // config_dir vazio pra rota de modelos, e nada re-dispararia quando os configs aterrissassem
        // (numa máquina com conta secundária, a lista viria reduzida ou da conta errada, sempre).
        carregarModelos();
      })
      // A lista de contas fora do ar NÃO pode deixar a tela com o estado da abertura passada — o
      // reset do `$effect` zera os campos, mas a lista de modelos só volta a ser pedida aqui (e no
      // onchange dos pickers). Com `selectedConfig` nulo, o backend normaliza pra conta padrão. A
      // guarda de geração é o que impede a rejeição de uma chamada SUPERADA (troca de servidor,
      // reabertura rápida) de apagar calado a escolha que o usuário fez depois dela.
      .catch(() => { if (seq === cfgSeq) carregarModelos(); });
    // Best-effort: falha aqui NAO pode travar a criação de sessão, so tira o seletor da tela.
    getEngines()
      .then((r) => { if (seq === cfgSeq) motores = r.motores; })
      .catch(() => { if (seq === cfgSeq) motores = {}; });
  }

  // Campo inline em vez de prompt(): o prompt nativo pode ser suprimido pelo navegador
  // ("impedir que esta página crie mais diálogos") e aí o clique vira nada, calado.
  let pedindoNome = $state(false);
  let nomeConta = $state('');

  function abrirCampoConta() {
    nomeConta = '';
    avisoConta = '';
    contaCriadaPath = null;
    contaErro = false;
    confirmandoApagar = false;
    pedindoNome = true;
  }

  // Só conta criada pelo hangar (`~/.claude-<nome>`) pode ser apagada, e nunca a que está em uso
  // pelo backend: o servidor recusa as duas com 409, mas oferecer o botão pra depois negar é pior
  // que não oferecer. `label` não serve de teste — o backend rotula pelo conteúdo, não pelo nome.
  const contaSelecionada = $derived(configs.find((c) => c.path === selectedConfig) ?? null);
  const nomeDaSelecionada = $derived(
    contaSelecionada && !contaSelecionada.active
      ? (contaSelecionada.path.split('/').pop() ?? '').match(/^\.claude-(.+)$/)?.[1] ?? null
      : null,
  );

  // Confirmação DENTRO da tela: `confirm()` nativo tem o mesmo defeito do `prompt()` — o navegador
  // pode suprimi-lo e aí apagar vira um clique que não faz nada, ou pior, faz sem perguntar.
  let confirmandoApagar = $state(false);

  async function apagar() {
    const nome = nomeDaSelecionada;
    if (!nome) return;
    // Caminho capturado ANTES do await, junto com o nome: `selectedConfig` é o que a tela mostra e
    // não pode ser a fonte da verdade depois que a operação começou.
    const apagadaPath = selectedConfig;
    const seq = ++cfgSeq;
    const serverId = targetServer;
    contaOcupada = true;
    avisoConta = '';
    contaErro = false;
    contaCriadaPath = null;
    try {
      await apagarConta(nome);
      if (seq !== cfgSeq || targetServer !== serverId) return;
      confirmandoApagar = false;
      let cs: ConfigDirInfo[];
      try {
        cs = await listClaudeConfigs();
      } catch {
        if (seq !== cfgSeq || targetServer !== serverId) return;
        // O DELETE JÁ deu certo: a pasta não existe mais. Deixar `selectedConfig` apontando pra ela
        // faria o "Nova sessão" mandar um config_dir apagado e levar 400 lá no backend. Tira da
        // lista local e volta pra ativa — sem depender do GET que acabou de falhar.
        configs = configs.filter((c) => c.path !== apagadaPath);
        selectedConfig = configs.find((c) => c.active)?.path ?? configs[0]?.path ?? null;
        avisoConta = `Conta ${nome} apagada, mas não consegui atualizar a lista — recarregue a tela.`;
        // B4 da revisão final: a seleção mudou de conta sem o onchange do combo passar — sem
        // recarregar aqui, o modelo/esforço da conta apagada sobreviveria e iria pro create.
        carregarModelos();
        return;
      }
      if (seq !== cfgSeq || targetServer !== serverId) return;
      configs = cs;
      // A seleção apontava pra pasta que sumiu: mandar esse caminho no create daria 400.
      selectedConfig = cs.find((c) => c.active)?.path ?? cs[0]?.path ?? null;
      avisoConta = `Conta ${nome} apagada.`;
      // B4 da revisão final: mesma regra do catch acima — seleção mudou por caminho
      // programático, o catálogo da conta que ficou tem que recarregar.
      carregarModelos();
    } catch (e) {
      if (seq !== cfgSeq || targetServer !== serverId) return;
      contaErro = true;
      avisoConta = e instanceof Error ? e.message : 'não consegui apagar a conta';
    } finally {
      if (seq === cfgSeq) contaOcupada = false;
    }
  }

  async function novaConta() {
    const nome = nomeConta.trim();
    if (!nome) {
      // Cancelou: encerra a intenção de criar. Feedback de tentativa anterior (sucesso ou erro)
      // não pode sobrar na tela como se valesse pro fluxo atual.
      avisoConta = '';
      contaCriadaPath = null;
      contaErro = false;
      return;
    }
    // Geração desta operação — a MESMA guarda do loadConfigs: um GET antigo (da abertura, ou do
    // servidor anterior) que aterrissar depois não pode sobrescrever a seleção da conta nova.
    // Sem isto, o loadConfigs em voo re-escolheria a conta ativa por cima do caminho criado.
    const seq = ++cfgSeq;
    const serverId = targetServer;
    contaOcupada = true;
    avisoConta = '';
    contaErro = false;
    try {
      const c = await criarConta(nome);
      if (seq !== cfgSeq || targetServer !== serverId) return;
      // Refresh em FASE PRÓPRIA: o POST já respondeu 200, então falha do GET não é falha da
      // criação — reportá-la como tal faria o usuário tentar de novo e levar 409.
      let cs: ConfigDirInfo[];
      try {
        cs = await listClaudeConfigs();
      } catch {
        if (seq !== cfgSeq || targetServer !== serverId) return;
        avisoConta = 'Conta criada, mas não consegui atualizar a lista — recarregue a tela.';
        return;
      }
      if (seq !== cfgSeq || targetServer !== serverId) return;
      configs = cs;
      // Exige que o caminho voltou do servidor antes de selecionar: selecionar um valor que não
      // está nas opções mandaria um config_dir órfão pro create() (400 do backend).
      const criada = cs.find((x) => x.path === c.path);
      if (criada) {
        pedindoNome = false;
        nomeConta = '';
        selectedConfig = criada.path;
        contaCriadaPath = criada.path;
        avisoConta = 'Conta criada. A sessão vai subir DESLOGADA — rode /login nela uma vez.';
        // B4 da revisão final: seleção mudou por caminho programático (sem onchange) — sem o
        // recarregar, o modelo da conta anterior iria pro create junto com a conta nova.
        carregarModelos();
      } else {
        contaCriadaPath = null;
        avisoConta = 'Conta criada, mas ainda não apareceu na lista — recarregue a tela.';
      }
    } catch (e) {
      if (seq !== cfgSeq || targetServer !== serverId) return;
      contaErro = true;
      avisoConta = e instanceof Error ? e.message : 'não consegui criar a conta';
    } finally {
      // Só a operação VIGENTE libera o botão: o finally de uma operação antiga (sheet fechado e
      // reaberto no meio do POST) não pode derrubar o estado da nova.
      if (seq === cfgSeq) contaOcupada = false;
    }
  }

  // Escape hatch: digitar o caminho na mao.
  let manualOpen = $state(false);
  let manualPath = $state('');

  // Zera tudo a cada abertura. Fixa o servidor-alvo (ativo atual ou o 1º) e o seleciona, pra o
  // scanner do passo 1 já varrer o backend certo.
  // IMPORTANTE: o reset depende SO de `open`. O corpo le `servers`/getActiveId DENTRO de untrack
  // senao o effect vira reativo a `servers` -> e a tela mae re-atribui `servers = listServers()`
  // a cada poll de 5s, o que re-disparava o reset no MEIO do fluxo (perdia a pasta escolhida e
  // voltava pro passo 1). untrack mantem o reset so na transicao de abertura.
  $effect(() => {
    if (!open) return;
    untrack(() => {
      picked = null;
      name = '';
      takenNames = new Set();
      hasSameFolder = false;
      error = '';
      checking = false;
      loading = false;
      manualOpen = false;
      manualPath = '';
      provider = 'claude';
      engine = '';
      // O sheet nunca desmonta (só o prop `open` alterna): sem este reset, a escolha da abertura
      // anterior sobrevive à reabertura quando o fetch de contas falha — o reset de carregarModelos
      // fica atrás dele e não roda. Escolha de Pi indo pro create do Claude é pane no ar e erro no
      // primeiro turno, calado.
      modelo = ''; esforco = '';
      modelos = []; listaReduzida = false; erroModelos = '';
      // Feedback da criação de conta não pode vazar entre aberturas: o botão liberado, o aviso
      // limpo e a conta criada esquecida — o cfgSeq do loadConfigs abaixo invalida qualquer
      // novaConta que ainda esteja em voo da abertura anterior.
      contaOcupada = false;
      avisoConta = '';
      contaCriadaPath = null;
      contaErro = false;
      const cur = getActiveId();
      const target = servers.find((s) => s.id === cur) ? cur! : servers[0]?.id ?? '';
      if (target) pickTarget(target);      // pickTarget ja carrega configs E motores do alvo
      else loadConfigs();                  // sem lista de servidores: carrega do ativo mesmo
    });
  });

  // Ao escolher a pasta: nome default = basename UNICO (sufixado se ja houver sessao com esse nome).
  // hasSameFolder so informa que ja existe sessao no cwd; NAO bloqueia criar outra.
  async function handlePick(p: string) {
    picked = p;
    error = '';
    checking = true;
    try {
      const sessions = await getSessions();
      takenNames = new Set(sessions.map((s) => s.name));
      hasSameFolder = sessions.some((s) => s.cwd === p);
      name = uniqueName(basename(p), takenNames);
    } catch {
      takenNames = new Set();
      hasSameFolder = false;
      name = basename(p);
    } finally {
      checking = false;
    }
  }

  function reset() {
    picked = null;
    takenNames = new Set();
    hasSameFolder = false;
    error = '';
  }

  function submitManual(e: SubmitEvent) {
    e.preventDefault();
    const p = manualPath.trim();
    if (p) handlePick(p);
  }

  async function create() {
    if (!picked || !name.trim()) return;
    loading = true;
    error = '';
    try {
      // Memória ANTES do onCreate: se a criação falhar (rede, 400), a escolha não se perde — o
      // valor lembrado é casado contra a lista na próxima abertura, então id de provedor que saiu
      // do ar nunca vira flag às cegas.
      // B6 da revisão final: campo em Padrão (vazio) REMOVE a chave — sem o removeItem, a
      // escolha "Padrão" nunca apagava a preferência antiga e a reabertura seguinte restaurava
      // o modelo que o usuário tinha acabado de descartar.
      if (modelo) localStorage.setItem(chaveMemoria(), modelo);
      else localStorage.removeItem(chaveMemoria());
      if (esforco) localStorage.setItem(chaveMemoria() + ':effort', esforco);
      else localStorage.removeItem(chaveMemoria() + ':effort');
      await onCreate(name.trim(), picked, provider === 'claude' ? selectedConfig : null, provider,
                     provider === 'claude' ? (engine || null) : null, modelo || null, esforco || null);
      onClose();
    } catch (err) {
      error = err instanceof Error ? err.message : 'Erro ao criar sessão';
    } finally {
      loading = false;
    }
  }

</script>

<BottomSheet {open} {onClose} ariaLabel="Nova sessão">
  <h2 class="sheet-title">Nova sessão</h2>

  {#if servers.length > 1}
    <div class="server-select">
      <span class="server-select-label">Servidor</span>
      <div class="server-chips">
        {#each servers as s (s.id)}
          <button
            type="button"
            class="server-chip"
            class:on={targetServer === s.id}
            style="--chip: {serverColor(s.id)};"
            onclick={() => pickTarget(s.id)}
            disabled={contaOcupada}
          >
            <span class="chip-dot" style="background: {serverColor(s.id)};" aria-hidden="true"></span>
            {s.label}
          </button>
        {/each}
      </div>
    </div>
  {/if}

  {#if !picked}
    <!-- Passo 1: escolher a pasta -->
    <!-- Remonta ao trocar de servidor: o scanner busca roots/pastas do server ATIVO só no onMount;
         sem o key ele ficava com as pastas do server anterior até fechar o app. -->
    {#key targetServer}
      <FolderScanner onPick={handlePick} />
    {/key}

    <div class="advanced">
      <button class="advanced-toggle" onclick={() => (manualOpen = !manualOpen)}>
        <span>Avançado: digitar caminho</span>
        <span class="chevron" class:chevron--open={manualOpen} aria-hidden="true">›</span>
      </button>
      {#if manualOpen}
        <form class="manual-form" onsubmit={submitManual}>
          <input
            type="text"
            class="field-input"
            bind:value={manualPath}
            placeholder="/home/voce/projetos/foo"
            autocomplete="off"
            autocorrect="off"
            autocapitalize="off"
            spellcheck={false}
            aria-label="Caminho do diretório"
          />
          <button type="submit" class="manual-go" disabled={!manualPath.trim()}>Usar</button>
        </form>
      {/if}
    </div>
  {:else}
    <!-- Passo 2: pasta escolhida -->
    <div class="picked">
      <div class="picked-head">
        <span class="picked-name">{basename(picked)}</span>
      </div>
      <span class="picked-path">{picked}</span>
    </div>

    {#if checking}
      <p class="hint">Verificando sessões…</p>
    {:else}
      {#if hasSameFolder}
        <p class="hint">Já há uma sessão nesta pasta — criando outra com nome único.</p>
      {/if}
      <div class="field">
        <label class="field-label" for="session-name">Nome</label>
        <input
          id="session-name"
          type="text"
          class="field-input"
          bind:value={name}
          placeholder="meu-projeto"
          autocomplete="off"
          autocorrect="off"
          autocapitalize="off"
          spellcheck={false}
          required
        />
      </div>

      <div class="field">
        <span class="field-label">Provider</span>
        <div class="provider-toggle" role="group" aria-label="Provider da sessão">
          {#each PROVIDERS as p (p)}
            <button
              type="button"
              class="provider-btn"
              class:on={provider === p}
              aria-pressed={provider === p}
              onclick={() => { provider = p; carregarModelos(); }}
            >{providerName(p)}</button>
          {/each}
        </div>
      </div>

      {#if provider === 'claude'}
        <div class="field">
          <label class="field-label" for="cfg-pick">Conta Claude</label>
          <div class="conta-row">
            <!-- disabled enquanto ocupada: trocar a conta com um apagar em voo fazia o texto da
                 confirmação mostrar um nome e a requisição apagar OUTRO (o nome foi capturado antes
                 do await), e a troca ainda era descartada calada no fim da operação. -->
            <Select id="cfg-pick" class="field-input" ariaLabel="Conta" disabled={contaOcupada}
              value={selectedConfig ?? ''}
              opcoes={configs.map((c) => ({
                value: c.path, label: c.label, hint: c.active ? 'atual' : undefined, title: c.path }))}
              onchange={(v) => { selectedConfig = v; carregarModelos(); }} />
            <button type="button" class="ghost-btn conta-add" onclick={abrirCampoConta}
              disabled={contaOcupada} aria-busy={contaOcupada}
              aria-label={contaOcupada ? 'Criando conta Claude' : 'Adicionar conta Claude'}>
              {contaOcupada ? '…' : '+ conta'}
            </button>
            {#if nomeDaSelecionada && !pedindoNome && !confirmandoApagar}
              <button type="button" class="ghost-btn conta-add" onclick={() => (confirmandoApagar = true)}
                disabled={contaOcupada}
                aria-label={`Apagar a conta ${nomeDaSelecionada}`}>apagar</button>
            {/if}
          </div>
          {#if confirmandoApagar && nomeDaSelecionada}
            <div class="conta-row conta-nova">
              <p class="conta-hint conta-confirma">
                Apagar <strong>{nomeDaSelecionada}</strong> e as conversas dela?
              </p>
              <button type="button" class="ghost-btn conta-add conta-perigo" onclick={apagar}
                disabled={contaOcupada}>{contaOcupada ? '…' : 'apagar'}</button>
              <button type="button" class="ghost-btn conta-add"
                onclick={() => (confirmandoApagar = false)} disabled={contaOcupada}>cancelar</button>
            </div>
          {/if}
          {#if pedindoNome}
            <div class="conta-row conta-nova">
              <!-- svelte-ignore a11y_autofocus -->
              <input class="field-input" type="text" autofocus bind:value={nomeConta}
                placeholder="nome da conta (minúsculas, números, - ou _)"
                aria-label="Nome da conta nova" disabled={contaOcupada}
                onkeydown={(e) => { if (e.key === 'Enter') { e.preventDefault(); novaConta(); }
                                    else if (e.key === 'Escape') pedindoNome = false; }} />
              <button type="button" class="ghost-btn conta-add" onclick={novaConta}
                disabled={contaOcupada || !nomeConta.trim()}>criar</button>
              <button type="button" class="ghost-btn conta-add" onclick={() => (pedindoNome = false)}
                disabled={contaOcupada}>cancelar</button>
            </div>
          {/if}
          {#if avisoConta && (!contaCriadaPath || selectedConfig === contaCriadaPath)}
            {#if contaErro}
              <p class="conta-hint" role="alert">{avisoConta}</p>
            {:else}
              <p class="conta-hint" role="status" aria-live="polite" aria-atomic="true">{avisoConta}</p>
            {/if}
          {/if}
        </div>
      {/if}

      {#if provider === 'claude' && Object.keys(motores).length}
        <div class="field">
          <label class="field-label" for="engine-pick">Motor</label>
          <Select id="engine-pick" ariaLabel="Motor" value={engine}
            opcoes={[{ value: '', label: 'Claude (sua conta)' },
                     ...Object.entries(motores).map(([nome, m]) => ({
                       value: nome, label: m.label ?? nome, hint: m.model }))]}
            onchange={(v) => { engine = v; carregarModelos(); }} />
        </div>
      {/if}

      {#if provider === 'claude' || provider === 'pi'}
        <div class="field">
          <label class="field-label" for="model-pick">Modelo</label>
          <Select id="model-pick" class="field-input" ariaLabel="Modelo" value={modelo}
            opcoes={[{ value: '', label: 'Padrão' },
                     ...modelos.map((m) => ({
                       value: valorModelo(m),
                       label: m.name ?? m.id,
                       // Quatro formatos do campo `models` (Task 4): pi traz provider/context/
                       // images, motor traz context_length/vision, cache do Claude traz name, e os
                       // aliases reduzidos não trazem nada. Campos ausentes simplesmente somem do
                       // hint (.filter(Boolean)) — nenhum formato pode quebrar a linha.
                       hint: [m.provider,
                              m.context ?? (m.context_length ? `${Math.round(m.context_length / 1000)}K` : null),
                              (m.vision ?? m.images) ? '👁' : null].filter(Boolean).join(' · ') }))]}
            onchange={(v) => (modelo = v)} />
          {#if listaReduzida}
            <p class="model-hint" role="status" aria-live="polite" aria-atomic="true">Lista reduzida — abra uma sessão nesta conta uma vez para o app aprender os modelos dela.</p>
          {/if}
          {#if erroModelos}
            <p class="model-hint" role="alert">{erroModelos} — a sessão abre no padrão.</p>
          {/if}
        </div>

        <div class="field">
          <label class="field-label" for="effort-pick">{provider === 'pi' ? 'Raciocínio' : 'Esforço'}</label>
          <Select id="effort-pick" class="field-input" ariaLabel={provider === 'pi' ? 'Raciocínio' : 'Esforço'} value={esforco}
            opcoes={[{ value: '', label: 'Padrão' },
                     ...NIVEIS[provider].map((n) => ({ value: n, label: n }))]}
            onchange={(v) => (esforco = v)} />
        </div>
      {/if}

      {#if error}
        <p class="error-msg" role="alert">{error}</p>
      {/if}

      <button class="primary-btn" onclick={create} disabled={loading || !name.trim()}>
        {loading ? 'Criando…' : 'Nova sessão'}
      </button>
      <button class="ghost-btn" onclick={reset}>Escolher outra pasta</button>
    {/if}
  {/if}
</BottomSheet>

<style>
  .sheet-title {
    font-size: var(--text-xl);
    font-weight: 600;
    color: var(--text-primary);
    margin-bottom: var(--space-4);
  }

  /* Seletor de servidor-alvo (só multi-servidor) */
  .server-select {
    margin-bottom: var(--space-4);
  }
  .server-select-label {
    display: block;
    font-size: var(--text-sm);
    color: var(--text-secondary);
    font-weight: 500;
    margin-bottom: var(--space-2);
  }
  .server-chips {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-2);
  }
  .server-chip {
    display: inline-flex;
    align-items: center;
    gap: var(--space-2);
    height: 34px;
    padding: 0 var(--space-3);
    border-radius: var(--radius-full);
    border: 1px solid var(--border-default);
    background: var(--bg-surface);
    color: var(--text-secondary);
    font-size: var(--text-sm);
    font-weight: 500;
    transition: border-color 160ms ease-out, color 160ms ease-out;
  }
  .server-chip.on {
    border-color: var(--chip);
    color: var(--text-primary);
  }
  .chip-dot {
    width: 8px; height: 8px; border-radius: 50%; flex-shrink: 0;
  }

  /* Toggle de provider (mesmo visual dos chips de servidor). */
  .provider-toggle {
    display: flex;
    gap: var(--space-2);
  }
  .provider-btn {
    height: 34px;
    padding: 0 var(--space-4);
    border-radius: var(--radius-full);
    border: 1px solid var(--border-default);
    background: var(--bg-surface);
    color: var(--text-secondary);
    font-size: var(--text-sm);
    font-weight: 500;
    transition: border-color 160ms ease-out, color 160ms ease-out;
  }
  .provider-btn.on {
    border-color: var(--accent);
    color: var(--text-primary);
  }

  /* ── Escape hatch: digitar caminho ─────────────────────────────────────── */
  .advanced {
    margin-top: var(--space-4);
    border-top: 1px solid var(--border-subtle);
    padding-top: var(--space-3);
  }

  .advanced-toggle {
    width: 100%;
    height: 44px;
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 0 var(--space-1);
    color: var(--text-secondary);
    font-size: var(--text-sm);
  }

  .chevron {
    color: var(--text-muted);
    transition: transform 180ms var(--ease-out);
  }
  .chevron--open {
    transform: rotate(90deg);
  }

  .manual-form {
    display: flex;
    gap: var(--space-2);
    margin-top: var(--space-2);
  }
  .manual-form .field-input {
    flex: 1;
  }
  .manual-go {
    flex-shrink: 0;
    height: 44px;
    padding: 0 var(--space-4);
    border-radius: var(--radius-md);
    background: var(--accent-dim);
    color: var(--text-primary);
    font-size: var(--text-sm);
    font-weight: 600;
  }
  .manual-go:disabled {
    opacity: 0.5;
  }

  /* ── Pasta escolhida ───────────────────────────────────────────────────── */
  .picked {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: var(--space-3);
    background: var(--bg-surface);
    border: 1px solid var(--border-subtle);
    border-radius: var(--radius-md);
    margin-bottom: var(--space-4);
  }

  .picked-head {
    display: flex;
    align-items: center;
    gap: var(--space-2);
  }

  .picked-name {
    font-size: var(--text-lg);
    font-weight: 600;
    color: var(--text-primary);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .picked-path {
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    color: var(--text-muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .hint {
    font-size: var(--text-sm);
    color: var(--text-secondary);
    margin-bottom: var(--space-3);
  }

  /* Avisos do campo de modelo: lista reduzida (cache do Claude frio) e erro de listagem. O erro
     termina com o caminho de saída (a sessão abre no padrão) — o usuário nunca fica sem ação. */
  .model-hint {
    margin: 6px 0 0;
    font-size: 12px;
    opacity: 0.75;
  }

  /* ── Campos / botoes ───────────────────────────────────────────────────── */
  .field {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    margin-bottom: var(--space-4);
  }

  .field-label {
    font-size: var(--text-sm);
    color: var(--text-secondary);
    font-weight: 500;
  }

  .field-input {
    height: 44px;
    background: var(--bg-surface);
    border: 1px solid var(--border-default);
    border-radius: var(--radius-md);
    color: var(--text-primary);
    font-family: var(--font-ui);
    font-size: 16px;
    padding: 0 var(--space-3);
    outline: none;
    transition: border-color 180ms var(--ease-out);
  }
  /* O combo de config é o <button> dentro do Select.svelte: CSS escopado não o alcança (o atributo
     de escopo só cai nos elementos deste template), então a regra acima nunca casava e ele saía com
     o visual padrão do componente — mono e 40px, quebrando o alinhamento com os campos irmãos. */
  :global(.sel-campo.field-input) {
    height: 44px;
    background: var(--bg-surface);
    border-radius: var(--radius-md);
    font-family: var(--font-ui);
  }
  .field-input::placeholder {
    color: var(--text-muted);
  }
  .field-input:focus {
    border-color: var(--accent);
    box-shadow: 0 0 0 2px var(--accent-dim);
  }

  /* Linha do seletor de conta + botão de cadastrar. O combo estica (flex: 1) e o botão fica
     fixo à direita; o aviso do /login vai embaixo. */
  .conta-row { display: flex; gap: 8px; align-items: center; }
  .conta-row :global(.field-input) { flex: 1; min-width: 0; }
  /* Especificidade de 2 classes: .ghost-btn (width:100%) vem DEPOIS no arquivo e, com uma classe
     só, venceria pela ordem da cascata — o botão comeria a linha e colapsaria o Select. */
  .conta-row .conta-add {
    flex: 0 0 auto;
    width: auto;
    height: 40px;                 /* mesma altura do .sel-campo, pra linha alinhar */
    margin-top: 0;
    padding: 0 var(--space-3);
    background: var(--surface-inset);
    border: 1px solid var(--border-default);
    border-radius: var(--radius-sm);
    color: var(--text-secondary);
    font-size: var(--text-sm);
    white-space: nowrap;
    transition: border-color 140ms var(--ease-out), color 140ms var(--ease-out);
  }
  .conta-row .conta-add:hover:not(:disabled) {
    border-color: var(--accent);
    color: var(--text-primary);
  }
  .conta-row .conta-add:disabled { opacity: 0.45; cursor: default; }
  .conta-nova { margin-top: var(--space-2); }
  .conta-confirma { flex: 1; min-width: 0; margin: 0; }
  .conta-row .conta-perigo { color: var(--error); border-color: var(--error); }
  .conta-row .conta-perigo:hover:not(:disabled) { color: var(--error); border-color: var(--error); }
  .conta-nova :global(.field-input) { height: 40px; }
  .conta-hint { margin: var(--space-2) 0 0; font-size: 12px; color: var(--text-secondary); }

  .error-msg {
    font-size: var(--text-sm);
    color: var(--error);
    margin-bottom: var(--space-3);
  }

  .primary-btn {
    width: 100%;
    height: 50px;
    background: var(--accent);
    border-radius: var(--radius-md);
    color: #fff;
    font-size: var(--text-base);
    font-weight: 600;
    transition: background 180ms var(--ease-out);
  }
  .primary-btn:active:not(:disabled) {
    background: var(--accent-press);
  }
  .primary-btn:disabled {
    opacity: 0.5;
    cursor: default;
  }

  .ghost-btn {
    width: 100%;
    height: 44px;
    margin-top: var(--space-2);
    color: var(--text-secondary);
    font-size: var(--text-sm);
    border-radius: var(--radius-md);
  }
  .ghost-btn:active {
    background: var(--bg-hover);
  }
</style>
