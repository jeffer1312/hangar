export type State = 'working' | 'idle' | 'awaiting_input' | 'dead';

export interface LoopHistoryEntry {
  n: number;
  ts: number;
  check_exit: number | null;
  tail: string;
}

export interface LoopState {
  goal: string;
  check_cmd: string | null;
  max_iters: number;
  require_branch: boolean;
  status: 'running' | 'paused_awaiting' | 'done_claimed' | 'done' | 'stopped' | 'exhausted' | 'failed';
  iter: number;
  history: LoopHistoryEntry[];
  started_ts: number;
  ended_ts: number | null;
  ended_reason: string | null;
}

// Qual Adapter dirige a sessao (app.adapters.get_adapter no backend). "claude" e o default;
// "codex" identifica as criadas via registry.create_codex; "pi" as detectadas pelo processo do
// pane (registry.provider_of_pane). O front usa isto pra esconder controles Claude-only (picker
// de /model, slash-commands) e pra rotular a sessao (lib/format.providerName).
// Tipo nomeado porque a mesma uniao viaja pela cadeia de criacao (CreateSessionSheet -> handleCreate
// das duas views -> api.createSession): quando o Pi entrou, as copias literais ficaram pra tras.
export type Provider = 'claude' | 'codex' | 'pi';

export interface SessionInfo {
  name: string;
  cwd?: string;
  jsonl?: string | null;
  provider?: Provider;
  state: State;
  last_activity?: number | null;
  // Vínculo nome<->transcript confiável? false = claude manual sem --session-id (chute mtime) ->
  // marca "sem id" e bloqueia o chat (evita mostrar/trocar a conversa errada).
  tracked?: boolean;
  branch?: string | null;   // branch git atual do cwd (mostrada na lista de sessões)
  // Estado vivo detalhado, pra a linha ser acionável sem abrir a sessão (feature #1):
  label?: string | null;       // working: texto do spinner
  question?: string | null;    // awaiting_input: a pergunta
  options?: string[] | null;   // awaiting_input: rótulos das opções
  // True quando "working" ha mais de CP_STALL_SECONDS sem avancar (feature #7: watchdog de travada) —
  // so tinge a linha; o backend (stall_watch.py) e quem decide o push.
  stalled?: boolean;
  // Feature #8 (rate-limit radar): banner de limite de uso detectado no pane (best-effort). limit_reset
  // = horario cru do reset ("3pm"/"15:30"), pro chip "limitado · HH:MM".
  limited?: boolean;
  limit_reset?: string | null;
  // Feature #12 (encadeamento de sessao): nome da sessao ALVO se ha um vinculo 'then' armado
  // ("quando terminar -> enviar pra"), null senao. So o alvo (indicador na lista); o texto do prompt
  // fica no backend, so exposto na hora de setar/limpar.
  then_target?: string | null;
  pair_peers?: string[] | null; // grupo de trabalho ativo: os OUTROS membros (2 sessões = lista de 1)
  pair_gid?: string | null;     // id estável do grupo (cluster da lista agrupa por ele)
  pair_task?: string | null;    // rótulo do grupo (ex: ABC-1234) pro header do cluster
  // Statusline crua da sessão (cache ~20s no backend) — o card do board/canvas parseia com
  // parseStatusLine (modelo/contexto no composer; ⚡5h/📅7d na RateStrip). Chat usa a versão ao vivo.
  status_line?: string | null;
  // Feature loop runner (Task 9+): estado de um loop autônomo por sessão
  loop_status?: LoopState['status'] | null;
  loop_iter?: number | null;    // iteração atual dentro do loop
  loop_max?: number | null;     // máximo de iterações permitidas
  // Progresso do plano do superpowers em execução nesta sessão (app/planprog.py). null = sem plano.
  plan_name?: string | null;        // nome do plano do superpowers em execução
  plan_task?: number | null;        // Task atual (ordinal, 1-based)
  plan_task_total?: number | null;
  plan_done?: number | null;        // steps marcados
  plan_total?: number | null;
  plan_complete?: boolean | null;
  plan_tasks?: [number, number][] | null;   // (done,total) por Task — alimenta a barra segmentada
  // Motor de modelo desta sessão (nome no engines.json). null/undefined = conta Anthropic.
  engine?: string | null;
}

// Detalhe do plano (GET /api/sessions/{name}/plan, Task 5): granularidade de Task/Step que o
// SessionInfo.plan_* não carrega (aquele é o resumo pra chip/barra na lista). 404 = sem plano ativo.
export interface PlanStep { title: string; done: boolean; manual: boolean }
export interface PlanTask { title: string; done: number; total: number; steps: PlanStep[] }
export interface PlanDetail {
  name: string; path: string;
  task: number; task_total: number;
  done: number; total: number; complete: boolean;
  tasks: PlanTask[];
  markdown: string;
}

// Sessão marcada com o servidor de origem (visão agregada multi-servidor).
export interface AggSession extends SessionInfo {
  serverId: string;
  serverLabel: string;
  serverColor: string;
}

// Candidato de resume: um transcript <uuid>.jsonl do cwd que a sessão "sem id" poderia retomar.
export interface ResumeCandidate {
  session_id: string;
  mtime?: number | null;
  preview: string;         // 1ª msg de usuário da conversa (pra reconhecer qual é)
  in_use: boolean;         // já é o transcript de outra sessão viva -> retomar aqui roubaria
}

// Resposta do /resume: ou a sessão já religada, ou (caso ambíguo) os candidatos pra confirmar.
export type ResumeResult = SessionInfo | { ambiguous: true; candidates: ResumeCandidate[] };

export interface ChatEvent {
  kind: 'user_msg' | 'assistant_msg' | 'tool_use' | 'tool_result';
  id: string;
  text?: string | null;
  tool_name?: string | null;
  tool_input?: Record<string, unknown> | null;
  tool_use_id?: string | null;
  result?: string | null;
  is_error?: boolean | null;
  ts?: number | null;
  // Cache de prompt (só em assistant_msg): tokens lidos do cache + janela de expiração em segundos.
  // O TTL vem medido do usage do transcript (1h ou 5min), não suposto.
  cache_read?: number | null;
  cache_ttl_s?: number | null;
  image_count?: number | null;   // imagens coladas no terminal -> busca lazy em /transcript-image
}

export interface StateEvent {
  session: string;
  state: State;
  label?: string | null;
  question?: string | null;
  options?: string[] | null;
  status_line?: string | null; // raw bottom chrome from the pane, shown as-is
  overlay?: boolean;            // overlay so-TUI aberto (/status, /config, /help, picker) -> espelho do pane
  login?: boolean;             // sessao na tela de welcome/login do Claude Code -> avisa + abre o espelho
  // Feature #8 (rate-limit radar): banner de limite de uso no pane (best-effort). limit_reset = horario
  // cru do reset ("3pm"/"15:30"), ou null.
  limited?: boolean;
  limit_reset?: string | null;
  // Loop runner: campos no SSE DA SESSÃO (chip do header do chat) — o Chat NÃO usa o
  // sessionsStore pra isso (reter o store no mobile abria N streams de lista e derrubava
  // a conexão do celular; regressão real revertida).
  loop_status?: LoopState['status'] | null;
  loop_iter?: number | null;
  loop_max?: number | null;
}

export interface CommandInfo {
  name: string;
  display: string;                 // forma exibida, ex: "/clear"
  description?: string | null;
  argumentHint?: string | null;    // dica de argumento, ex: "<ambiente>"
  source: 'builtin' | 'skill' | 'plugin';
  destructive?: boolean;           // exige confirmação antes de enviar
}

// ── Workflows (painel estilo /workflows do terminal) ────────────────────────
export interface WorkflowSummary {
  runId: string;
  name: string;
  status: string; // completed | killed | running
  agentCount: number;
  phaseCount: number;
  totalTokens: number;
  durationMs: number;
  startTime: number;
  running: boolean;
}

export interface WorkflowAgent {
  agentId: string | null;
  label: string | null;
  phaseTitle: string | null;
  state: string | null; // done | error | progress
  model: string | null;
  tokens: number;
  durationMs: number;
  toolCalls: number;
  lastToolName: string | null;
  lastToolSummary: string | null;
  resultPreview: string | null;
}

// Subagente solto (tool Agent), lido do transcript dele em <session-dir>/subagents/.
export interface SubagentRun {
  agentId: string;
  agentType: string | null;
  prompt: string | null;
  startedAt: string;
  updatedAt: string;
  mtime: number;
  toolCalls: number;
  tools: { name: string; count: number }[];
  recent: { name: string; target: string }[];
  lastText: string;
  // Transcript do subagente nos MESMOS ChatEvent do chat (so com ?events=N).
  events?: ChatEvent[];
}

export interface WorkflowAgentDetail {
  agentId: string;
  label: string;
  phaseTitle: string | null;
  state: string | null;
  model: string | null;
  tokens: number;
  durationMs: number;
  toolCalls: number;
  prompt: string | null;
  result: string | null;
  tools: { name: string; count: number }[];
}

export interface WorkflowDetail {
  runId: string;
  name: string;
  status: string;
  totalTokens: number;
  durationMs: number;
  summary: string | null;
  phases: { title: string | null; detail: string | null }[];
  agents: WorkflowAgent[];
}

// ── Config dirs do Claude (~/.claude, alternativas) ────────────────────────
export interface ConfigDirInfo {
  path: string;
  label: string;
  active: boolean;
}

// ── Scanner de pastas ───────────────────────────────────────────────────────
export interface FsRoot {
  name: string;   // basename da raiz (vira o rótulo do chip)
  path: string;   // caminho absoluto da raiz liberada
}

export interface FsEntry {
  name: string;
  path: string;
  is_git: boolean;
  has_claude_md: boolean;
  mtime?: number | null;
}

// Estado de falha da varredura, mapeado pra uma mensagem visível na UI.
export type FsScanError =
  | 'permission_denied'
  | 'unreadable'
  | 'root_not_allowed'
  | 'invalid_path'
  | 'not_found'
  | 'unknown';

export interface FsScanResult {
  entries: FsEntry[];
  error?: FsScanError | null;
}

// ── AskUserQuestion (stepper nativo multi-pergunta) ─────────────────────────
export interface AskOption { label: string; description: string; preview?: string }
export interface AskQuestionItem { header: string; question: string; multiSelect: boolean; options: AskOption[] }
export interface AskQuestionPayload { questions: AskQuestionItem[] }
export type AnswerItem =
  | { kind: 'option'; indices: number[]; multi: boolean; labels: string[] }
  | { kind: 'text'; value: string; type_index: number; labels: string[] }
  | { kind: 'chat'; chat_index: number };

// ── Custos (visão agregada de uso/gasto por conta) ──────────────────────────
export interface CostBucket {
  key: string;
  sessions: number;
  input: number;
  output: number;
  cache_read: number;
  cache_write: number;
  cost: number;
}

// ── Custos v2 (dashboard multi-agente: Claude Code + Codex + Pi) ────────────
// Um corte qualquer (dia, provedor, fonte, projeto, modelo). `key` é o valor da dimensão.
// O custo vem quebrado por tipo DENTRO do corte: é o que faz a barra do projeto mostrar a FORMA
// do gasto (projeto de output != projeto de cache) sem precisar clicar.
export interface DimBucket {
  key: string;
  // Rótulo legível quando a `key` não serve pra ler — hoje só a conta Anthropic, cuja chave é
  // 'anthropic:<uuid>' (identidade que não colide na malha) mas cujo nome é o e-mail. A tela
  // exibe `label ?? key`; ausente é o caso normal, em que a chave já é o nome.
  label?: string | null;
  sessions: number;
  input: number;
  output: number;
  cache_write: number;
  cache_read: number;
  cost: number;
  cost_input: number;
  cost_output: number;
  cost_cache_write: number;
  cost_cache_read: number;
}

// Uma linha por combinação que REALMENTE aconteceu (dia × provedor × fonte × projeto × modelo ×
// subagente). É o que os `by_*` perdem ao somar: depois de somado não dá pra separar "quanto
// daquele projeto foi de tal fonte". Esparso na prática — o número de combinações que existem é
// uma fração ínfima do produto das dimensões.
export interface ComboRow {
  dia: string;
  provider: string;
  source: string;
  project: string;
  model: string;
  subagente: boolean;
  sessions: number;
  input: number;
  output: number;
  cache_write: number;
  cache_read: number;
  cost: number;
  cost_input: number;
  cost_output: number;
  cost_cache_write: number;
  cost_cache_read: number;
}

export interface KindBucket {
  kind: string; // "input" | "output" | "cache_write" | "cache_read"
  tokens: number;
  cost: number;
}

export interface RateInfo {
  model: string;
  provider: string;
  input: number;
  output: number;
  cache_read: number;
  cache_write: number;
  origin: string; // "override" | "models.dev" | "snapshot"
  cache_estimado?: boolean;
}

// Eco dos filtros que o servidor REALMENTE aplicou. FastAPI ignora query param desconhecido: um
// backend antigo recebe ?period=7d e devolve TUDO. Sem este eco o front somaria "7 dias da
// máquina A" com "sempre da máquina B" e mostraria como um período só.
export interface Applied {
  period: string;
}

// Formato COMPLETO — o que mergeReports produz e a tela consome. A tolerância a servidor antigo
// mora na ENTRADA (ServerResult usa Partial<CostReport>), não aqui: deixar tudo opcional nos dois
// lados obrigaria a tela inteira a `?? 0` num dado que o merge garante existir.
export interface CostReport {
  totals: DimBucket;
  by_day: DimBucket[];
  by_provider: DimBucket[];
  by_source: DimBucket[];
  by_project: DimBucket[];
  by_model: DimBucket[];
  by_kind: KindBucket[];
  rates: RateInfo[];
  sem_tarifa: string[];
  // Quanto custaria se NENHUM token fosse cache (cache W e R a preço de input cheio).
  custo_sem_cache: number;
  // Tokens em "equivalente-input": cada tipo pesado pela PRÓPRIA tarifa. Vem do servidor porque
  // depende da tarifa de cada modelo, que o front não tem.
  equivalente_cobrado: number;
  // Totais da janela imediatamente ANTERIOR, do mesmo tamanho — a régua do "subiu ou desceu".
  // null quando não dá pra comparar (period=all, ou janela anterior com pouco registro).
  anterior: DimBucket | null;
  // Detalhamento cruzado. OPCIONAL porque servidor da malha em versão antiga não manda: sem ele a
  // tela cai no recorte de uma dimensão só, a partir dos `by_*`.
  combos?: ComboRow[];
  applied: Applied;
  usd_brl: number | null;
}

export interface Runner {
  label: string;
  command: string;
  source: 'npm' | 'make' | 'stack';
  is_dev_guess: boolean;
}

export interface RunInfo {
  command: string;
  since?: number | null;
}

export interface RunnersResponse {
  detected: Runner[];
  remembered: string | null;
  running: RunInfo | null;
}

// Janela de rate-limit normalizada (GET /limits, Task B) — so Codex; ver docs/codex-app-server-contract.md.
export interface RateLimitWindow {
  usedPercent: number | null;
  windowMins: number | null;
  resetsAt: number | null; // epoch em SEGUNDOS
}

export interface SessionLimits {
  primary: RateLimitWindow | null;
  secondary: RateLimitWindow | null;
  planType: string | null;
}

// Modelo + reasoning effort do Codex (GET /models, Task C) — normalizado no backend a partir de
// `model/list` (hidden ja filtrado). effort e string livre (o que vier de supportedReasoningEfforts).
export interface CodexModelEffort {
  value: string;
  description: string | null;
}

export interface CodexModel {
  model: string;
  displayName: string | null;
  description: string | null;
  efforts: CodexModelEffort[];
  defaultEffort: string | null;
}

export interface CodexModelChoice {
  model: string | null;
  effort: string | null;
}

export interface CodexModelsResponse {
  models: CodexModel[];
  current: CodexModelChoice;
}

// Modelo + nivel de raciocinio de uma sessao Pi (GET /pi/models). Vem do sidecar que a extensao
// cp-state.ts publica perguntando pro proprio Pi — nao ha picker raspado aqui (o /model do Pi e
// uma lista com busca de ~300 modelos). `levels` sao os niveis que o modelo ATUAL aceita: variam
// por modelo (medido: glm-5.2 -> off/low/medium/high/xhigh; k3 -> low/high/max).
export interface PiModel {
  provider: string;
  id: string;
  name: string;
  reasoning: boolean;
}

export interface PiModelsResponse {
  models: PiModel[];
  current: { provider: string; id: string; name?: string } | null;
  thinking: string | null;
  levels: string[];
}

// Um anexo já enviado pra sessão (GET /api/sessions/{name}/uploads) — a galeria lista o diretório
// <cwd>/.claude-pocket-uploads. expires_in_days vem do backend (só ele conhece o prazo de retenção);
// null = retenção desligada e PODE ser <= 0: o prune só roda no próximo upload, então um anexo
// vencido continua aparecendo até lá.
export interface UploadFile {
  filename: string;
  size: number;
  mtime: number;            // epoch em SEGUNDOS
  expires_in_days: number | null;
}
