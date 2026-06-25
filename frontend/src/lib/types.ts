export type State = 'working' | 'idle' | 'awaiting_input' | 'dead';

export interface SessionInfo {
  name: string;
  cwd?: string;
  jsonl?: string | null;
  state: State;
  last_activity?: number | null;
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
