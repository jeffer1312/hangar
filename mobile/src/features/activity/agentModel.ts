import type { AgentRun, ChatEvent, SubagentRun } from '@hangar/core';
export { matchAgentSubagent } from '@hangar/core';

export function agentIdentities(events: ChatEvent[]): Map<string, string> {
  const ids = new Map<string, string>();
  for (const event of events) {
    if (event.kind !== 'tool_result' || !event.tool_use_id) continue;
    const launched = event.bg_agent_id;
    const id = launched?.startsWith('teammate:')
      ? event.result?.match(/agent_id:\s*([A-Za-z0-9_-]+)/)?.[1]
      : launched ?? event.result?.match(/agentId:\s*([A-Za-z0-9_-]+)/)?.[1];
    if (id) ids.set(event.tool_use_id, id);
  }
  return ids;
}

export function agentModel(agent: AgentRun, sub: SubagentRun | undefined): string | undefined {
  // Registro sem modelo não comprova que o apelido pedido chegou a rodar.
  return sub ? sub.model : agent.model;
}
