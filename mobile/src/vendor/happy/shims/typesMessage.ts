// Shim para '@/sync/typesMessage' — só o que o vendor tools/ consome
export type ToolCall = {
  name: string;
  state: 'running' | 'completed' | 'error';
  input: any;
  createdAt: number;
  startedAt: number | null;
  completedAt: number | null;
  description: string | null;
  result?: any;
  permission?: {
    id: string;
    status: 'pending' | 'approved' | 'denied' | 'canceled';
    reason?: string;
    mode?: string;
    allowedTools?: string[];
    decision?: 'approved' | 'approved_for_session' | 'denied' | 'abort' | string;
    date?: number;
  };
};

export type ToolCallMessage = {
  kind: 'tool-call';
  id: string;
  localId: string | null;
  createdAt: number;
  tool: ToolCall;
  children: Message[];
};

export type Message =
  | ToolCallMessage
  | { kind: 'user-text' | 'agent-text'; id: string; localId: string | null; createdAt: number; text: string }
  | { kind: 'agent-event'; id: string; createdAt: number; event: unknown };
