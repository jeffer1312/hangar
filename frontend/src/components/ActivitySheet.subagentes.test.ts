// @vitest-environment happy-dom
// Subagentes que só o DISCO conhece, no painel de Atividade.
//
// O defeito que estes testes travam (18/08/2026): o painel listava apenas os subagentes derivados
// do TRANSCRIPT — `tool_use` com nome `Agent`. Quem forka hoje é a SKILL (`plugin:kubectl`), que
// entra no transcript como `Skill`, e o agente de FUNDO, que não entra como ferramenta nenhuma.
// Resultado medido numa sessão real do usuário: 3 subagentes no disco, 0 no painel, e a tela
// dizendo "nada rolando agora" com os arquivos deles ali do lado — os dados já vinham de uma rota
// que existia desde sempre (`GET /api/sessions/{name}/subagents`).
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { mount, unmount, tick } from 'svelte';
import ActivitySheet from './ActivitySheet.svelte';
import * as m from '../paraglide/messages';
import * as apiLib from '../lib/api';
import type { SubagentRun } from '@hangar/core';

vi.mock('../lib/api', () => ({
  getPermissionModes: vi.fn().mockResolvedValue({ current: 'plan', modes: ['plan', 'auto', 'manual', 'acceptEdits'] }),
  setPermissionMode: vi.fn().mockResolvedValue({ mode: 'plan', current: 'plan' }),
  isTimeoutError: vi.fn(() => false),
  isAbortError: vi.fn(() => false),
  getWorkflows: vi.fn(async () => []),
  getWorkflow: vi.fn(),
  getWorkflowAgent: vi.fn(),
  getSubagents: vi.fn(async () => []),
  getSubagent: vi.fn(async (_n: string, id: string) => ({ ...sub(id), events: [] })),
}));
const api = vi.mocked(apiLib);

function sub(agentId: string, over: Partial<SubagentRun> = {}): SubagentRun {
  return {
    agentId,
    agentType: 'general-purpose',
    prompt: 'Base directory for this skill: /home/u/.claude/plugins/grafana\n\n# Grafana - Skill\n'
      + 'Skill para operacoes com Grafana.',
    startedAt: '2026-08-18T14:00:00Z',
    updatedAt: '2026-08-18T14:02:00Z',
    mtime: 1787076000,
    toolCalls: 22,
    tools: [{ name: 'Bash', count: 22 }],
    recent: [{ name: 'Bash', target: 'kubectl get pods' }],
    lastText: '',
    ...over,
  };
}

// Atividade derivada do transcript: VAZIA — é o cenário exato do defeito.
const SEM_ATIVIDADE = { tasks: [], agents: [], inProgress: 0, runningAgents: 0 };

function montar(atividade: unknown = SEM_ATIVIDADE) {
  const el = document.createElement('div');
  document.body.appendChild(el);
  const comp = mount(ActivitySheet, {
    target: el,
    props: { open: true, sessionName: 'jefferson-2', activity: atividade, onClose: vi.fn() } as never,
  });
  return { el, comp: comp as never };
}

beforeEach(() => { vi.clearAllMocks(); api.getWorkflows.mockResolvedValue([]); });
afterEach(() => { document.body.innerHTML = ''; });

describe('ActivitySheet — subagentes do disco', () => {
  it('lista o subagente que o transcript não menciona', async () => {
    api.getSubagents.mockResolvedValue([sub('aab93a1d3'), sub('a74ae5150')]);
    const t = montar();
    await tick(); await tick(); await tick();
    expect(document.body.textContent).toContain(m.atividade_subagentes());
    // O título sai da primeira linha ÚTIL do prompt: o cabeçalho "Base directory for this skill:"
    // e as linhas de markdown não dizem nada sobre o que ele foi fazer.
    expect(document.body.textContent).toContain('Skill para operacoes com Grafana');
    expect(document.body.textContent).not.toContain('Base directory for this skill');
    unmount(t.comp);
  });

  it('com subagente no disco, o painel NÃO diz que não há nada', async () => {
    api.getSubagents.mockResolvedValue([sub('aab93a1d3')]);
    const t = montar();
    await tick(); await tick(); await tick();
    expect(document.body.textContent).not.toContain(m.atividade_vazio());
    unmount(t.comp);
  });

  it('sem nada em lugar nenhum, o vazio continua aparecendo', async () => {
    api.getSubagents.mockResolvedValue([]);
    const t = montar();
    await tick(); await tick(); await tick();
    expect(document.body.textContent).toContain(m.atividade_vazio());
    unmount(t.comp);
  });

  it('mostra quantas ferramentas o subagente já chamou', async () => {
    api.getSubagents.mockResolvedValue([sub('aab93a1d3', { toolCalls: 26 })]);
    const t = montar();
    await tick(); await tick(); await tick();
    expect(document.body.textContent).toContain(m.atividade_chamadas({ n: 26 }));
    unmount(t.comp);
  });

  it('clicar abre o transcript DAQUELE agente, sem casar por prompt', async () => {
    // Dois subagentes com o MESMO prompt (é o caso comum: a mesma skill chamada duas vezes). O
    // casamento por texto acertaria sempre o primeiro; aqui a linha já carrega o objeto.
    api.getSubagents.mockResolvedValue([sub('primeiro'), sub('segundo')]);
    const t = montar();
    await tick(); await tick(); await tick();
    const linhas = [...document.querySelectorAll<HTMLButtonElement>('.agent-row.openable')];
    expect(linhas).toHaveLength(2);
    linhas[1].click();
    await tick(); await tick();
    expect(api.getSubagent).toHaveBeenCalledWith('jefferson-2', 'segundo', 200);
    unmount(t.comp);
  });
});
