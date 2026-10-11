// Escolhas e carregadores da tela inicial de nova conversa (celular). Reimplementa só o pedaço do
// CreateSessionSheet que esta tela usa — duplicação aceita: a folha carrega bastão, motor, contas
// novas e retomada, que aqui não existem.
import {
  createSessionForServer, sendInputForServer, fetchSessionsForServer, getCodexAccountsForServer,
  getFolderBranchesForServer, getRootsForServer, listClaudeConfigs, getClaudeAccountSuggestion, getProviders,
  uniqueSessionName, basename, effortLevels, defaultCodexAccount, SESSION_PROVIDERS,
  getEnginesForServer, getConfigForServer, contextModel, hasContext, setClaudeDefaultsForServer,
  getWorktreesForServer, getCreationProgress, uploadFileForServer,
  type WorktreeStatus, type CreationProgress,
  type CodexAccount, type ConfigDirInfo, type FolderBranches, type FsRoot, type ModelOption, type Provider,
  type WorktreeChoice, type Motor, type CliProxyAccount,
} from '@hangar/core';
import { listOwnServers, selectServer, getActiveId, type Server } from './auth';
import { carregarModelos, valorModelo } from './modelosPorConta';
import { bestAccountWithQuota, exhaustedWindow, type ContaCota } from './cota';
import * as m from '../paraglide/messages';

type ProviderProbe = Record<string, { disponivel: boolean; motivo: string | null; default?: boolean }>;

const errText = (e: unknown, fallback: string) => (e instanceof Error && e.message ? e.message : fallback);
export function isNotRepo(e: unknown): boolean {
  const status = (e as { status?: number } | null)?.status;
  return status === 404 || (status === 409 && errText(e, '').includes('not a git repository'));
}
export function worktreeChoiceOf(d: { branch: string; newBranch: boolean; base: string; branchName: string }): WorktreeChoice | null {
  if (d.newBranch && d.branchName.trim()) return { branch: d.branchName.trim(), new_branch: true, base: d.base || null };
  return d.branch ? { branch: d.branch } : null;
}

const cwdKey =(server: string) => `cp_newchat_cwd:${server}`;

function readStorage(key: string): string | null {
  try { return localStorage.getItem(key); } catch { return null; }
}
function writeStorage(key: string, value: string) {
  try {
    if (value) localStorage.setItem(key, value);
    else localStorage.removeItem(key);
  } catch { /* storage bloqueado: segue sem memória */ }
}

export class NewChatDraft {
  servers = $state<Server[]>([]);
  server = $state('');
  cwd = $state<string | null>(null);
  provider = $state<Provider>('claude');
  configDir = $state<string | null>(null);
  codexAccount = $state('');
  model = $state('');
  effort = $state('');
  branch = $state('');
  newBranch = $state(false);
  base = $state('');
  branchName = $state('');
  // Worktree que já existe escolhida como destino (create.rs: `existing`, só na tela compacta).
  existing = $state<string | null>(null);
  worktrees = $state<WorktreeStatus[]>([]);
  // Anexos escolhidos antes de a sessão existir: sobem na sessão nova, logo depois de criá-la.
  attachments = $state<File[]>([]);
  progress = $state('');
  sameFolder = $state(false);
  #sameSeq = 0;
  // Motor GPT do proxy, conta ChatGPT, Fast e 1M: as mesmas escolhas da folha (CreateSessionSheet).
  engine = $state('');
  engines = $state<Record<string, Motor>>({});
  enginesError = $state('');
  engineAccount = $state('');
  fastChoice = $state<'default' | 'priority' | null>(null);
  contextOn = $state(false);
  permission = $state('bypassPermissions');
  // null = config do servidor ainda não lida: o corpo não manda o campo e vale o padrão do servidor.
  headless = $state<boolean | null>(null);
  executionError = $state('');
  // Padrão marcado com "Usar como padrão" (o harness-defaults.json do nativo, aqui no localStorage).
  savedDefault = $state<{ model: string; effort: string; permission: string } | null>(null);
  #permissionTouched = false;
  // Última leitura de cota; corta as contas de backup da pílula de conta.
  #quotaLine = $state<ContaCota[] | null>(null);

  providers = $state<ProviderProbe>({});
  providersLoading = $state(false);
  providersError = $state('');
  roots = $state<FsRoot[] | null>(null);
  rootsError = $state('');
  configs = $state<ConfigDirInfo[]>([]);
  configsLoading = $state(false);
  configsError = $state('');
  codexAccounts = $state<CodexAccount[]>([]);
  codexLoading = $state(false);
  codexError = $state('');
  models = $state<ModelOption[]>([]);
  modelsLoading = $state(false);
  modelsError = $state('');
  branches = $state<FolderBranches | null>(null);
  branchesLoading = $state(false);
  branchesError = $state('');

  sending = $state(false);
  sendError = $state('');

  // Um contador por carregador: resposta de servidor/conta/pasta já trocados é descartada.
  #provSeq = 0;
  #rootsSeq = 0;
  #cfgSeq = 0;
  #codexSeq = 0;
  #modSeq = 0;
  #branchSeq = 0;
  #engSeq = 0;
  #execSeq = 0;
  #modelTouched = false;
  #providerPicked = false;
  // Só a escolha manual de CONTA desliga a troca automática por cota.
  #accountPicked = false;
  // Sessão já criada cujo primeiro envio falhou: tentar de novo reenvia nela em vez de criar outra,
  // desde que nenhuma escolha tenha mudado desde então.
  #created: { choices: string; name: string } | null = null;

  get serverObj(): Server | null { return this.servers.find((s) => s.id === this.server) ?? null; }
  get #choices(): string {
    return JSON.stringify([this.server, this.cwd, this.provider, this.configDir, this.codexAccount, this.model, this.effort, this.branch,
      this.newBranch, this.base, this.branchName, this.existing, this.engine, this.engineAccount, this.fastChoice, this.contextOn,
      this.permission, this.headless]);
  }
  /** Contas ou modelos ainda chegando: enviar agora mandaria conta/modelo vazios e cairia no padrão do servidor calado. */
  get loading(): boolean {
    return this.providersLoading || this.configsLoading || this.codexLoading || this.modelsLoading
      || (this.headless === null && !this.executionError);
  }
  get motor(): Motor | null { return this.provider === 'claude' && this.engine ? this.engines[this.engine] ?? null : null; }
  // Só conta de credencial Codex do Hangar serve: é dela que sai a cota e o login do proxy.
  get proxyAccounts(): CliProxyAccount[] {
    return (this.motor?.cliproxy_accounts ?? []).filter((a) => !!a.account && /^codex:./.test(a.credential_id));
  }
  get #chosen(): ModelOption | null { return this.models.find((x) => valorModelo(x) === this.model) ?? null; }
  get fastAvailable(): boolean {
    const mod = this.#chosen;
    if (!mod) return false;
    return this.provider === 'codex'
      ? !!mod.service_tiers?.some((t) => t.id === 'priority' && !t.hidden)
      : this.provider === 'claude' && this.proxyAccounts.length > 0 && !!mod.supports_fast;
  }
  // 1M só existe junto do Fast do motor (choices.rs render_engine_context).
  get contextAvailable(): boolean { return this.provider === 'claude' && this.fastAvailable; }
  get bodyModel(): string | null { return this.model ? contextModel(this.model, this.contextOn && this.contextAvailable) : null; }
  // O que impede abrir no motor do proxy, na ordem do nativo; '' = pronto.
  get proxyBlocked(): string {
    const motor = this.motor;
    if (!motor) return '';
    if (motor.cliproxy_error) return m.native_create_proxy_error({ reason: motor.cliproxy_error });
    if (!motor.cliproxy_accounts) return '';
    if (!motor.cliproxy_accounts.length) return m.native_create_proxy_no_accounts();
    if (!this.proxyAccounts.some((a) => a.account === this.engineAccount)) return m.native_create_proxy_choose_account();
    if (!this.modelsLoading && !this.modelsError && !this.models.length) return m.native_create_proxy_no_models();
    return '';
  }
  // Contas que o /api/cotas conhece, mais a ativa (choices.rs:245); sem leitura, todas.
  get accountChoices(): ConfigDirInfo[] {
    const line = this.#quotaLine;
    if (!line) return this.configs;
    return this.configs.filter((c) => c.active || c.path === this.configDir || line.some((q) => q.id === `claude:${c.path}`));
  }
  get levels(): readonly string[] { return effortLevels(this.provider, this.models, this.model); }
  get providerAvailable(): boolean { return this.providers[this.provider]?.disponivel !== false; }

  init() {
    this.servers = listOwnServers();
    const active = getActiveId();
    const first = this.servers.some((s) => s.id === active) ? active! : (this.servers[0]?.id ?? '');
    if (first) this.pickServer(first);
  }

  pickServer(id: string) {
    this.#providerPicked = false;
    this.#modelTouched = false;
    this.server = id;
    selectServer(id);
    this.cwd = readStorage(cwdKey(id));
    this.branch = ''; this.newBranch = false; this.base = ''; this.branchName = ''; this.existing = null;
    this.engines = {}; this.engine = ''; this.engineAccount = '';
    void this.checkSameFolder();
    void this.loadProviders();
    void this.loadRoots();
    void this.loadEngines();
    void this.loadExecution();
    this.loadAccounts();
    void this.loadBranches();
  }

  async loadEngines() {
    const seq = ++this.#engSeq;
    const server = this.serverObj;
    this.enginesError = '';
    if (!server) return;
    try {
      const r = await getEnginesForServer(server);
      if (seq === this.#engSeq) this.engines = r.motores ?? {};
    } catch (e) {
      if (seq === this.#engSeq) this.enginesError = errText(e, m.falha_conexao());
    }
  }

  async loadExecution() {
    const seq = ++this.#execSeq;
    const server = this.serverObj;
    this.headless = null;
    this.executionError = '';
    if (!server) return;
    try {
      const r = await getConfigForServer(server);
      if (seq === this.#execSeq) this.headless = r.campos.headless_default?.valor !== false;
    } catch (e) {
      if (seq === this.#execSeq) this.executionError = errText(e, m.falha_conexao());
    }
  }

  // Permissão de cada provider como o nativo (create.rs set_provider); motor e janela são do provider anterior.
  #resetForProvider() {
    this.#permissionTouched = false;
    this.permission = this.provider === 'codex' ? 'Full Access' : this.provider === 'claude' ? 'bypassPermissions' : '';
    this.engine = ''; this.engineAccount = ''; this.fastChoice = null; this.contextOn = false;
  }

  setEngine(name: string) {
    this.#modelTouched = true;
    if (name === this.engine) return;
    this.engine = name;
    this.engineAccount = this.proxyAccounts.find((a) => a.account === this.engineAccount)?.account ?? this.proxyAccounts[0]?.account ?? '';
    this.fastChoice = null; this.contextOn = false;
    void this.loadModels();
  }

  setEngineAccount(id: string) {
    this.#modelTouched = true;
    if (id === this.engineAccount) return;
    this.engineAccount = id;
    void this.loadModels();
  }

  setContext(on: boolean) { this.contextOn = on; }

  setPermission(value: string) { this.#permissionTouched = true; this.permission = value; }

  // Chave do nativo (default_key): servidor:provider:motor[:account:conta ChatGPT]; sem a conta Codex,
  // porque o padrão vale "para todas as contas".
  harnessKey(): string {
    return `cp_harness_default:${this.server}:${this.provider}:${this.engine || '-'}`
      + (this.provider === 'claude' && this.engineAccount ? `:account:${this.engineAccount}` : '');
  }
  get isDefault(): boolean {
    const d = this.savedDefault;
    return !!d && d.model === (this.bodyModel ?? '') && d.effort === this.effort && d.permission === this.permission;
  }
  async saveDefault(on: boolean) {
    if (!on) { writeStorage(this.harnessKey(), ''); this.savedDefault = null; return; }
    const saved = { model: this.bodyModel ?? '', effort: this.effort, permission: this.permission };
    writeStorage(this.harnessKey(), JSON.stringify(saved));
    this.savedDefault = saved;
    // Só Claude na conta Anthropic grava no settings.json da máquina (o Rust recusa modelo de motor).
    if (this.provider === 'claude' && !this.engine && this.serverObj) {
      try {
        await setClaudeDefaultsForServer(this.serverObj, { model: saved.model || null, effort: saved.effort || null, permission: saved.permission || null });
      } catch (e) { this.sendError = m.native_create_default_failed({ erro: errText(e, m.falha_conexao()) }); }
    }
  }
  #readDefault(): { model: string; effort: string; permission: string } | null {
    try {
      const raw = readStorage(this.harnessKey());
      const d = raw ? JSON.parse(raw) : null;
      return d && typeof d.model === 'string' ? { model: d.model, effort: String(d.effort ?? ''), permission: String(d.permission ?? '') } : null;
    } catch { return null; }
  }

  setCwd(path: string) {
    this.cwd = path;
    this.branch = ''; this.newBranch = false; this.base = ''; this.branchName = ''; this.existing = null;
    writeStorage(cwdKey(this.server), path);
    void this.loadBranches();
    void this.checkSameFolder();
  }

  // Pasta que já tem sessão: avisa, sem travar (o nome novo desempata), como o `same_folder` do nativo.
  async checkSameFolder() {
    const seq = ++this.#sameSeq;
    const server = this.serverObj, cwd = this.cwd;
    this.sameFolder = false;
    if (!server || !cwd) return;
    try {
      const list = await fetchSessionsForServer(server);
      if (seq === this.#sameSeq) this.sameFolder = list.some((x) => x.cwd === cwd);
    } catch { /* a lista da sessão já mostra a máquina fora; aqui só não avisa */ }
  }

  pickExisting(path: string | null) {
    this.existing = path;
    if (path) { this.branch = ''; this.newBranch = false; }
  }

  setProvider(p: Provider) {
    this.#providerPicked = true;
    if (p === this.provider) return;
    this.provider = p;
    this.#resetForProvider();
    this.loadAccounts();
  }

  setConfig(path: string) {
    this.#modelTouched = true;
    this.#accountPicked = true;
    if (path === this.configDir) return;
    this.configDir = path;
    void this.loadModels();
  }

  /** Conta escolhida sem limite e sem escolha manual: troca sozinha pra conta com folga. */
  switchFromExhausted(linha: ContaCota[] | null) {
    this.#quotaLine = linha;
    if (this.provider !== 'claude' || this.#accountPicked || this.configsLoading || !this.configDir) return;
    const atual = linha?.find((c) => c.id === `claude:${this.configDir}`);
    if (!exhaustedWindow(atual)) return;
    const alvo = bestAccountWithQuota(linha, this.configs.map((c) => c.path));
    if (!alvo || alvo === this.configDir) return;
    this.configDir = alvo;
    void this.loadModels();
  }

  setCodexAccount(id: string) {
    this.#modelTouched = true;
    if (id === this.codexAccount) return;
    this.codexAccount = id;
    void this.loadModels();
  }

  setModel(value: string) {
    this.#modelTouched = true;
    this.model = value;
    if (!this.levels.includes(this.effort)) this.effort = '';
    if (!this.fastAvailable) { this.fastChoice = null; this.contextOn = false; }
  }

  // Mesma chave da folha (`CreateSessionSheet`): a escolha feita num lugar vale no outro.
  memoryKey(): string {
    return `cp_last_model:${this.server}:${this.provider}:${this.provider === 'codex' ? this.codexAccount : this.engine || '-'}`
      + (this.provider === 'claude' && this.engineAccount ? `:account:${this.engineAccount}` : '');
  }

  async loadProviders() {
    const seq = ++this.#provSeq;
    this.providers = {};
    this.providersLoading = true;
    this.providersError = '';
    try {
      const res = await getProviders();
      if (seq !== this.#provSeq) return;
      this.providers = res;
      // Sem padrão marcado, volta ao Claude: o provedor do servidor anterior não vale aqui.
      const preferred = SESSION_PROVIDERS.find((provider) => res[provider]?.default && res[provider]?.disponivel) ?? 'claude';
      if (!this.#providerPicked && !this.#modelTouched && !this.sending && preferred !== this.provider) {
        this.provider = preferred;
        this.#resetForProvider();
        this.loadAccounts();
      }
    } catch (e) {
      if (seq === this.#provSeq) this.providersError = errText(e, m.criar_providers_erro());
    } finally {
      if (seq === this.#provSeq) this.providersLoading = false;
    }
  }

  async loadRoots() {
    const seq = ++this.#rootsSeq;
    const server = this.serverObj;
    this.roots = null;
    this.rootsError = '';
    if (!server) return;
    try {
      const res = await getRootsForServer(server);
      if (seq === this.#rootsSeq) this.roots = res;
    } catch (e) {
      if (seq === this.#rootsSeq) this.rootsError = errText(e, m.falha_conexao());
    }
  }

  loadAccounts() {
    // O modelo é da conta/provider anterior: invalida a lista em voo e limpa a escolha já, para um
    // catálogo atrasado (ou a falha da conta nova) não deixar modelo de outro provider no envio.
    ++this.#cfgSeq; ++this.#codexSeq; ++this.#modSeq;
    this.models = []; this.model = ''; this.effort = ''; this.modelsLoading = false; this.modelsError = '';
    this.configs = []; this.configDir = null; this.configsLoading = false; this.configsError = '';
    this.codexAccounts = []; this.codexAccount = ''; this.codexLoading = false; this.codexError = '';
    if (this.provider === 'claude') void this.loadConfigs();
    else if (this.provider === 'codex') void this.loadCodex();
    else void this.loadModels();
  }

  async loadConfigs() {
    const seq = ++this.#cfgSeq;
    this.#modelTouched = false;
    this.#accountPicked = false;
    this.configsLoading = true;
    try {
      const list = await listClaudeConfigs();
      if (seq !== this.#cfgSeq) return;
      this.configs = list;
      this.configDir = list.find((c) => c.active)?.path ?? list[0]?.path ?? null;
      this.configsLoading = false;
      const modelGen = this.#modSeq + 1;
      void this.loadModels();
      // Conta sugerida pela cota, como a folha: nunca passa por cima de uma escolha já feita.
      getClaudeAccountSuggestion().then(({ path }) => {
        if (seq !== this.#cfgSeq || modelGen !== this.#modSeq || this.#modelTouched || this.provider !== 'claude'
            || !this.configs.some((c) => c.path === path) || path === this.configDir) return;
        this.configDir = path;
        void this.loadModels();
      }).catch(() => { /* sem leitura de cota, fica a conta ativa */ });
    } catch (e) {
      if (seq !== this.#cfgSeq) return;
      this.configsLoading = false;
      this.configsError = errText(e, m.falha_conexao());
      void this.loadModels();
    }
  }

  async loadCodex() {
    const seq = ++this.#codexSeq;
    const server = this.serverObj;
    if (!server) return;
    this.codexLoading = true;
    try {
      const list = await getCodexAccountsForServer(server);
      if (seq !== this.#codexSeq) return;
      this.codexAccounts = list;
      this.codexAccount = defaultCodexAccount(list)?.id ?? '';
      this.codexLoading = false;
      void this.loadModels();
    } catch (e) {
      if (seq !== this.#codexSeq) return;
      this.codexLoading = false;
      this.codexError = errText(e, m.falha_conexao());
    }
  }

  async loadModels() {
    const seq = ++this.#modSeq;
    this.models = []; this.model = ''; this.effort = ''; this.modelsError = '';
    const server = this.serverObj;
    if (this.provider === 'codex' && (!this.codexAccount || !server)) { this.modelsLoading = false; return; }
    this.modelsLoading = true;
    try {
      const claude = this.provider === 'claude';
      const r = await carregarModelos({
        provider: this.provider, configDir: claude ? this.configDir : null,
        engine: claude ? this.engine || null : null,
        engineAccount: claude && this.engine ? this.engineAccount || null : null,
        ...(this.provider === 'codex' ? { codexAccount: this.codexAccount, server } : {}),
      }, this.memoryKey());
      if (seq !== this.#modSeq) return;
      this.models = r.models;
      this.model = r.lembrado;
      this.contextOn = r.contextoLembrado;
      this.effort = this.levels.includes(r.esforcoLembrado) ? r.esforcoLembrado : '';
      // O padrão marcado vence o último modelo (choices.rs load_models), se ainda estiver na lista.
      const saved = this.#readDefault();
      this.savedDefault = saved;
      const base = saved ? contextModel(saved.model, false) : '';
      if (saved && (!base || r.models.some((x) => valorModelo(x) === base))) {
        this.model = base;
        this.contextOn = hasContext(saved.model);
        this.effort = this.levels.includes(saved.effort) ? saved.effort : '';
        if (saved.permission && !this.#permissionTouched) this.permission = saved.permission;
      }
    } catch (e) {
      if (seq === this.#modSeq) this.modelsError = errText(e, m.criar_modelos_erro());
    } finally {
      if (seq === this.#modSeq) this.modelsLoading = false;
    }
  }

  async loadBranches() {
    const seq = ++this.#branchSeq;
    const server = this.serverObj, cwd = this.cwd;
    this.branches = null;
    this.branchesError = '';
    if (!server || !cwd) { this.branchesLoading = false; return; }
    this.branchesLoading = true;
    this.worktrees = [];
    // Worktrees desta pasta, para o menu de branch oferecer as que já existem como destino.
    getWorktreesForServer(server, undefined, { repo: cwd, sizes: false })
      .then((repos) => {
        if (seq === this.#branchSeq) this.worktrees = repos.flatMap((r) => r.worktrees).filter((w) => w.exists && w.path !== cwd);
      })
      .catch(() => { /* sem worktrees, o menu mostra só as branches */ });
    try {
      const res = await getFolderBranchesForServer(server, cwd);
      if (seq === this.#branchSeq) this.branches = res;
    } catch (e) {
      // Pasta fora de repositório não é falha: só não há pílula de branch (mesma regra do nativo).
      if (seq === this.#branchSeq && !isNotRepo(e)) this.branchesError = errText(e, m.falha_conexao());
    } finally {
      if (seq === this.#branchSeq) this.branchesLoading = false;
    }
  }

  /** Aviso embaixo das pílulas, na prioridade do `note()` do nativo. */
  get note(): { text: string; warning: boolean } | null {
    if (this.sending) return { text: this.progress || m.native_new_chat_sending(), warning: false };
    const claude = this.provider === 'claude';
    const text = [
      this.sendError,
      this.proxyBlocked,
      this.enginesError && `${m.native_create_engine()}: ${this.enginesError}`,
      this.executionError && m.newchat_execution_failed({ erro: this.executionError }),
      this.rootsError && `${m.native_create_roots_failed()} ${this.rootsError}`,
      claude ? this.configsError : '',
      this.provider === 'codex' ? this.codexError : '',
      this.modelsError && `${m.native_create_models_failed()}: ${this.modelsError}`,
      this.branchesError && m.native_create_checkout_failed({ reason: this.branchesError }),
      this.roots?.length === 0 ? m.native_new_chat_no_roots() : '',
      claude && !this.configsLoading && !this.configsError && this.configs.length === 0 && this.#cfgSeq > 0
        ? m.native_new_chat_no_accounts() : '',
      !this.providerAvailable ? m.native_create_provider_missing({ p: this.provider }) : '',
      this.sameFolder ? m.criar_ja_existe() : '',
      this.providersError && m.criar_providers_erro_detalhe({ erro: this.providersError }),
    ].find(Boolean);
    return text ? { text, warning: true } : null;
  }

  // Passo da criação a cada 800 ms (STEP_POLL do nativo) enquanto o POST espera.
  #followCreation(name: string, server: Server): () => void {
    let alive = true;
    this.progress = '';
    void (async () => {
      while (alive) {
        try {
          const p = await getCreationProgress(name, server);
          if (alive) this.progress = stepLabel(p) || this.progress;
        } catch { /* consulta que falha deixa o último passo; a criação reporta o erro dela */ }
        await new Promise((r) => setTimeout(r, 800));
      }
    })();
    return () => { alive = false; this.progress = ''; };
  }

  async send(text: string): Promise<{ serverId: string; name: string }> {
    if (this.sending) throw new Error(m.native_new_chat_sending());
    this.sendError = '';
    const server = this.serverObj, cwd = this.cwd;
    try {
      if (!server || !cwd) throw new Error(m.newchat_sem_pasta());
      if (!this.providerAvailable) throw new Error(m.native_create_provider_missing({ p: this.provider }));
      if (this.loading) throw new Error(m.comum_carregando());
      if (this.provider === 'codex' && !this.codexAccount) throw new Error(m.newchat_sem_conta_codex());
      this.sending = true;
      const choices = this.#choices;
      let name = this.#created?.choices === choices ? this.#created.name : '';
      if (!name) {
        const taken = new Set((await fetchSessionsForServer(server)).map((s) => s.name));
        // Memória antes de criar, como a folha: a escolha não se perde se a criação falhar.
        const key = this.memoryKey();
        writeStorage(key, this.bodyModel ?? '');
        writeStorage(`${key}:effort`, this.effort);
        const target = this.existing ?? cwd;
        const sessionName = uniqueSessionName(basename(target), taken);
        if (this.newBranch && !this.branchName.trim()) this.branchName = sessionName;
        // Corpo do nativo (create.rs, create): motor, conta ChatGPT, Fast, execução e permissão.
        const claude = this.provider === 'claude', codex = this.provider === 'codex';
        const tier = this.fastAvailable ? this.fastChoice : null;
        const stopProgress = this.#followCreation(sessionName, server);
        const info = await createSessionForServer(server, {
          name: sessionName, cwd: target, provider: this.provider,
          // Sonda falhada deixa o Claude por omissão, e isso não é escolha a lembrar.
          remember_provider: this.#providerPicked || Object.keys(this.providers).length > 0,
          config_dir: claude ? this.configDir : null,
          codex_account: codex ? this.codexAccount : undefined,
          engine: claude ? this.engine || null : null,
          ...(claude && this.engine && this.engineAccount ? { engine_account: this.engineAccount } : {}),
          model: this.bodyModel, effort: this.effort || null,
          ...(tier ? { service_tier: tier } : {}),
          ...((claude || codex) && this.headless !== null ? { headless: this.headless } : {}),
          ...(claude && this.permission ? { permission_mode: this.permission } : {}),
          // O backend só aceita permissão no Codex sem terminal (api.py, _create_session_owned).
          ...(codex && this.headless === true ? { permission_mode: this.permission || null } : {}),
          // Worktree existente já é o cwd: branch nova ou outra branch não se somam a ela.
          ...(this.existing ? {} : (worktreeChoiceOf(this) ?? {})),
        }).finally(stopProgress);
        // O nome que vale é o devolvido pelo backend: ele pode desempatar de novo.
        name = info.name;
        // Releitura das escolhas: o nome da branch nova pode ter sido preenchido acima.
        this.#created = { choices: this.#choices, name };
      }
      const parts: string[] = [];
      for (const file of this.attachments) {
        // Um anexo que falha segura a mensagem: ela não sai pela metade (create.rs, CreatedWithInput).
        const { path } = await uploadFileForServer(server, name, file);
        parts.push((file.type.startsWith('image/') ? `📎 ${m.board_imagem()}: ` : `📎 ${m.board_arquivo()}: `) + path);
      }
      await sendInputForServer(server, name, [text, ...parts].filter(Boolean).join('\n'));
      this.attachments = [];
      this.#created = null;
      return { serverId: server.id, name };
    } catch (e) {
      this.sendError = errText(e, m.criar_sessao_erro());
      throw e;
    } finally {
      this.sending = false;
    }
  }
}

function stepLabel(p: CreationProgress): string {
  switch (p.step) {
    case 'preparando': return m.criar_passo_preparando();
    case 'conta': return m.criar_passo_conta({ conta: p.params.conta ?? '' });
    case 'criando': return m.criar_passo_criando();
    default: return '';
  }
}

export function createNewChatDraft(): NewChatDraft {
  return new NewChatDraft();
}
