import type { AgentRun, SubagentRun } from '@hangar/core';
export { agentIdentities, matchAgentSubagent } from '@hangar/core';

export function agentModel(agent: AgentRun, sub: SubagentRun | undefined): string | undefined {
  // Registro sem modelo não comprova que o apelido pedido chegou a rodar.
  return sub ? sub.model : agent.model;
}
