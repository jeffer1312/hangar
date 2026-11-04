import { expect, it } from 'vitest';
import { codexSubagentActivityKey, projectCodexSubagent } from './codexSubagents';
import { projectSubagentActivity } from './activity';
import type { Activity } from './activity';
import type { CodexSubagent, SubagentRun } from './types';

const run: SubagentRun = { agentId: 'child', agentPath: '/root/review', agentType: null, prompt: null,
  startedAt: '', updatedAt: '', mtime: 0, toolCalls: 0, tools: [], recent: [], lastText: '', state: 'running' };
const view: CodexSubagent = { agent_path: '/root/review', state: 'finished', ended_at: 1000, can_message: false, message_blocked: null };

it('a atividade muda por evento sem trocar UUID/caminho e independe da ordem do mapa', () => {
  const initial = { child: { ...view, activity_revision: 1 }, other: view };
  expect(codexSubagentActivityKey(initial)).toBe(codexSubagentActivityKey({ other: view, child: initial.child }));
  expect(codexSubagentActivityKey(initial)).not.toBe(codexSubagentActivityKey({ ...initial, child: { ...view, activity_revision: 2 } }));
  expect(codexSubagentActivityKey({ child: view })).toBe(codexSubagentActivityKey({ child: { ...view, activity_revision: 0 } }));
});

it('o estado novo vence uma descoberta ou detalhe atrasado e permite retomada', () => {
  const finished = projectCodexSubagent(run, { child: view });
  expect(finished.state).toBe('finished');
  expect(finished.endedAt).toBe('1970-01-01T00:16:40.000Z');
  const resumed = projectCodexSubagent(finished, { child: { ...view, state: 'running', ended_at: null } });
  expect([resumed.state, resumed.endedAt]).toEqual(['running', null]);
  // Retrato terminado sem hora mantém a hora do disco.
  expect(projectCodexSubagent(finished, { child: { ...view, ended_at: null } }).endedAt).toBe('1970-01-01T00:16:40.000Z');
  expect(projectCodexSubagent(run, {}).state).toBe('running');
  expect(projectCodexSubagent(run, { other: view })).toBe(run);
  expect(projectCodexSubagent(run, { child: { ...view, state: null } }).problema).toBe('erro_subagente_estado_indisponivel');
});

it('o mapa decide o estado pelo caminho mesmo antes de os metadados chegarem', () => {
  const activity: Activity = { tasks: [], shells: [], agents: [{ id: 'spawn', kind: 'agent', description: 'review', agentPath: '/root/review', running: true }], total: 0, done: 0, inProgress: 0, runningAgents: 1, runningShells: 0 };
  expect(projectSubagentActivity(activity, [], undefined, { child: view }).runningAgents).toBe(0);
});

it.each(['running', 'finished', 'failed'] as const)('a falta de sinal nativo preserva %s conhecido no disco', state => {
  const disk: SubagentRun = { ...run, state, finished: state !== 'running', failed: state === 'failed',
    endedAt: state === 'running' ? null : '1970-01-01T00:16:40.000Z', canMessage: true };
  const unavailable: CodexSubagent = { ...view, state: null, ended_at: null, can_message: false,
    message_blocked: 'erro_subagente_mensagem_codex_indisponivel' };
  const projected = projectCodexSubagent(disk, { child: unavailable });
  expect(projected).toMatchObject({ state: disk.state, finished: disk.finished, failed: disk.failed, endedAt: disk.endedAt,
    canMessage: false, messageBlocked: unavailable.message_blocked, problema: 'erro_subagente_estado_indisponivel' });
  const activity: Activity = { tasks: [], shells: [], agents: [{ id: 'spawn', kind: 'agent', description: 'review', agentId: 'child', running: state !== 'running' }],
    total: 0, done: 0, inProgress: 0, runningAgents: state === 'running' ? 0 : 1, runningShells: 0 };
  expect(projectSubagentActivity(activity, [projected], undefined, { child: unavailable }).runningAgents).toBe(state === 'running' ? 1 : 0);
});

it('sem sinal nativo nem estado no disco não inventa execução ou conclusão', () => {
  const unknown: SubagentRun = { ...run, state: null };
  const unavailable: CodexSubagent = { ...view, state: null, ended_at: null };
  const projected = projectCodexSubagent(unknown, { child: unavailable });
  expect(projected.state).toBeNull();
  expect(projected.finished).toBeUndefined();
  expect(projected.problema).toBe('erro_subagente_estado_indisponivel');
  const activity: Activity = { tasks: [], shells: [], agents: [{ id: 'spawn', kind: 'agent', description: 'review', agentId: 'child', running: false }],
    total: 0, done: 0, inProgress: 0, runningAgents: 0, runningShells: 0 };
  expect(projectSubagentActivity(activity, [projected], undefined, { child: unavailable })).toBe(activity);
});

it.each([true, false])('sem estado nativo preserva o estado %s já conhecido pelo transcript', running => {
  const activity: Activity = { tasks: [], shells: [], agents: [{ id: 'spawn', kind: 'agent', description: 'review', agentId: 'child', running }],
    total: 0, done: 0, inProgress: 0, runningAgents: running ? 1 : 0, runningShells: 0 };
  expect(projectSubagentActivity(activity, [], undefined, { child: { ...view, state: null } })).toBe(activity);
});
