import type { CodexSubagents, SubagentRun } from './types';

export function codexSubagentState(states: CodexSubagents | undefined, id?: string | null, path?: string | null) {
  if (id) return states?.[id];
  return path ? Object.values(states ?? {}).find(state => state.agent_path === path) : undefined;
}

export function projectCodexSubagent(run: SubagentRun, states: CodexSubagents | undefined): SubagentRun {
  const state = codexSubagentState(states, run.agentId, run.agentPath);
  if (!state) return run;
  const ended = new Date(state.ended_at === null ? NaN : state.ended_at * 1000);
  return { ...run, agentPath: state.agent_path ?? run.agentPath, state: state.state,
    finished: state.state ? state.state !== 'running' : undefined, failed: state.state === 'failed',
    endedAt: Number.isFinite(ended.getTime()) ? ended.toISOString() : null,
    canMessage: state.can_message, messageBlocked: state.message_blocked,
    problema: state.state ? undefined : 'erro_subagente_estado_indisponivel' };
}
