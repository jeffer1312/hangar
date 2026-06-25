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
