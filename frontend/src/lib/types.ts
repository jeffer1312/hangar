export type State = 'working' | 'idle' | 'awaiting_input' | 'dead';

export interface SessionInfo {
  name: string;
  cwd?: string;
  jsonl?: string | null;
  state: State;
  last_activity?: number | null;
}

// Sessão marcada com o servidor de origem (visão agregada multi-servidor).
export interface AggSession extends SessionInfo {
  serverId: string;
  serverLabel: string;
  serverColor: string;
}

export interface ChatEvent {
  kind: 'user_msg' | 'assistant_msg' | 'tool_use' | 'tool_result';
  id: string;
  parent_id?: string | null;
  text?: string | null;
  tool_name?: string | null;
  tool_input?: Record<string, unknown> | null;
  tool_use_id?: string | null;
  result?: string | null;
  is_error?: boolean | null;
  ts?: number | null;
}

export interface StateEvent {
  session: string;
  state: State;
  label?: string | null;
  question?: string | null;
  options?: string[] | null;
  status_line?: string | null; // raw bottom chrome from the pane, shown as-is
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
