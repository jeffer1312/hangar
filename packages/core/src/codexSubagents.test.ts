import { expect, it } from 'vitest';
import { projectCodexSubagent } from './codexSubagents';
import { projectSubagentActivity } from './activity';
import type { Activity } from './activity';
import type { CodexSubagent, SubagentRun } from './types';

const run: SubagentRun = { agentId: 'child', agentPath: '/root/review', agentType: null, prompt: null,
  startedAt: '', updatedAt: '', mtime: 0, toolCalls: 0, tools: [], recent: [], lastText: '', state: 'running' };
const view: CodexSubagent = { agent_path: '/root/review', state: 'finished', ended_at: 1000, can_message: false, message_blocked: null };

it('o estado novo vence uma descoberta ou detalhe atrasado e permite retomada', () => {
  const finished = projectCodexSubagent(run, { child: view });
  expect(finished.state).toBe('finished');
  expect(finished.endedAt).toBe('1970-01-01T00:16:40.000Z');
  expect(projectCodexSubagent(finished, { child: { ...view, state: 'running', ended_at: null } }).state).toBe('running');
  expect(projectCodexSubagent(run, {}).state).toBe('running');
  expect(projectCodexSubagent(run, { other: view })).toBe(run);
  expect(projectCodexSubagent(run, { child: { ...view, state: null } }).problema).toBe('erro_subagente_estado_indisponivel');
});

it('o mapa decide o estado pelo caminho mesmo antes de os metadados chegarem', () => {
  const activity: Activity = { tasks: [], shells: [], agents: [{ id: 'spawn', kind: 'agent', description: 'review', agentPath: '/root/review', running: true }], total: 0, done: 0, inProgress: 0, runningAgents: 1, runningShells: 0 };
  expect(projectSubagentActivity(activity, [], undefined, { child: view }).runningAgents).toBe(0);
});
