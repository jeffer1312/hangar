import { describe, it, expect } from 'vitest';
import { createActivityFolder } from './activity';
import type { ChatEvent } from './types';

let seq = 0;
// Agent (launch bloqueante em background) -> vira uma linha no painel.
const launch = (tuid: string): ChatEvent =>
  ({ kind: 'tool_use', id: `e${seq++}`, tool_name: 'Agent', tool_use_id: tuid, tool_input: { description: 'Assessment A — design review' } });
// tool_result imediato do launch: traz o agentId no texto "Async agent launched...".
const launched = (tuid: string, agentId: string): ChatEvent =>
  ({ kind: 'tool_result', id: `e${seq++}`, tool_use_id: tuid, result: `Async agent launched successfully. agentId: ${agentId} (internal ID)` });
// fim real: <task-notification> numa user_msg com o <task-id> = agentId.
const done = (agentId: string): ChatEvent =>
  ({ kind: 'user_msg', id: `e${seq++}`, text: `<task-notification>\n<task-id>${agentId}</task-id>\n<status>completed</status>\n</task-notification>` });

function run(events: ChatEvent[]) {
  const f = createActivityFolder();
  f.reset(events);
  return f.snapshot();
}

describe('activity — pareamento de agente background', () => {
  it('marca terminado quando o fim vem DEPOIS do launch (ordem normal)', () => {
    const s = run([launch('tu1'), launched('tu1', 'aa0777'), done('aa0777')]);
    expect(s.runningAgents).toBe(0);
    expect(s.agents[0].running).toBe(false);
  });

  it('marca terminado quando o fim vem ANTES do launch (regressao: troca de transcript no /clear)', () => {
    // o <task-notification> do fim e processado no fold ANTES do tool_result do launch. Antes do fix
    // (pareamento dependente de ordem) o par nunca fechava e o agente ficava "rodando" pra sempre.
    const s = run([launch('tu1'), done('aa0777'), launched('tu1', 'aa0777')]);
    expect(s.runningAgents).toBe(0);
  });

  it('segue rodando enquanto nao chega o evento de fim', () => {
    const s = run([launch('tu1'), launched('tu1', 'aa0777')]);
    expect(s.runningAgents).toBe(1);
    expect(s.agents[0].running).toBe(true);
  });
});
