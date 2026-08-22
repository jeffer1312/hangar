import type { ChatEvent } from '@hangar/core';
import type { ToolCall } from '../vendor/happy/shims/typesMessage';

const ms = (ts?: number | null): number => (ts ? ts * 1000 : Date.now());

// ChatEvent.ts é epoch em SEGUNDOS (backend/app/transcript.py:_ts)
// Happy's ToolCall usa ms (Date.now style)
export function toToolCall(use: ChatEvent, result?: ChatEvent): ToolCall {
  return {
    name: use.tool_name ?? '?',
    input: use.tool_input ?? {},
    createdAt: ms(use.ts),
    startedAt: ms(use.ts),
    completedAt: result ? ms(result.ts) : null,
    description: null,
    state: !result ? 'running' : result.is_error ? 'error' : 'completed',
    result: result?.result ?? undefined,
  };
}

export function pairTools(events: ChatEvent[]): Map<string, { use: ChatEvent; result?: ChatEvent }> {
  const m = new Map<string, { use: ChatEvent; result?: ChatEvent }>();
  for (const e of events) {
    if (!e.tool_use_id) continue;
    if (e.kind === 'tool_use') {
      const cur = m.get(e.tool_use_id);
      if (cur) cur.use = e;
      else m.set(e.tool_use_id, { use: e });
    } else if (e.kind === 'tool_result') {
      const cur = m.get(e.tool_use_id);
      if (cur) cur.result = e;
      else m.set(e.tool_use_id, { use: e, result: e });
    }
  }
  return m;
}
