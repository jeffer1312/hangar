export type State = 'working' | 'idle' | 'awaiting_input' | 'dead';

export interface SessionInfo {
  name: string;
  cwd?: string;
  jsonl?: string | null;
  // Qual Adapter dirige a sessao (app.adapters.get_adapter no backend). "claude" cobre toda
  // sessao de hoje; "codex" identifica as criadas via registry.create_codex — o front usa isto
  // pra esconder controles Claude-only (picker de /model, slash-commands) e mostrar o badge.
  provider?: 'claude' | 'codex';
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
export interface AskOption { label: string; description: string }
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

export interface CostModelBucket {
  model: string;
  sessions: number;
  cost: number;
}

export interface AccountCost {
  account_id: string;
  email: string | null;
  label: string;
  totals: CostBucket;
  today: number;
  yesterday: number;
  by_day: CostBucket[];
  by_week: CostBucket[];
  by_month: CostBucket[];
  by_model: CostModelBucket[];
}

export interface CostReport {
  accounts: AccountCost[];
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
