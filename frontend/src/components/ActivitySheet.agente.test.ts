// @vitest-environment happy-dom
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { mount, unmount, tick } from 'svelte';
import ActivitySheet from './ActivitySheet.svelte';
import * as apiLib from '../lib/api';

vi.mock('../lib/api', () => ({
  getPermissionModes: vi.fn().mockResolvedValue({ current: 'plan', modes: [] }),
  setPermissionMode: vi.fn(),
  isTimeoutError: vi.fn(() => false),
  isAbortError: vi.fn(() => false),
  getWorkflows: vi.fn(async () => []),
  getWorkflow: vi.fn(),
  getWorkflowAgent: vi.fn(),
  getSubagents: vi.fn(async () => []),
  getSubagent: vi.fn(async () => ({ events: [] })),
}));
const api = vi.mocked(apiLib);

const SEM_ATIVIDADE = { tasks: [], agents: [], inProgress: 0, runningAgents: 0 };

function montar() {
  const el = document.createElement('div');
  document.body.appendChild(el);
  const comp = mount(ActivitySheet, {
    target: el,
    props: { open: true, sessionName: 'test', activity: SEM_ATIVIDADE, onClose: vi.fn() } as never,
  });
  return { el, comp: comp as never };
}

beforeEach(() => { vi.clearAllMocks(); });
afterEach(() => { document.body.innerHTML = ''; });

describe('ActivitySheet — detalhe do agente (T6)', () => {
  it('renderiza prompt e resultado como markdown (sem ** cru)', async () => {
    const prompt = '1. **Ordem e dependências:** a sequência T1→T5 se sustenta?';
    const result = '#### Bloqueia Task\n- **A receita do PATH quebra no fish.**';
    api.getWorkflows.mockResolvedValue([{ runId: 'wf1', name: 'Test', agentCount: 1, phaseCount: 0, totalTokens: 0, durationMs: 0, startTime: 0, running: false }]);
    api.getWorkflow.mockResolvedValue({
      runId: 'wf1', name: 'Test', status: 'completed', totalTokens: 0, durationMs: 0, summary: null,
      phases: [], agents: [{ agentId: 'a1', label: 'Agente', phaseTitle: null, state: 'done', model: 'claude-sonnet', tokens: 100, durationMs: 1000, toolCalls: 3, lastToolName: 'Grep', lastToolTarget: 'test', lastToolSummary: null, resultPreview: null }],
    });
    api.getWorkflowAgent.mockResolvedValue({
      agentId: 'a1', label: 'Agente', phaseTitle: null, state: 'done', model: 'claude-sonnet', tokens: 100, durationMs: 1000, toolCalls: 3,
      prompt, result, tools: [{ name: 'Bash', count: 2 }, { name: 'Read', count: 1 }], lastToolName: 'Grep', lastToolTarget: 'test',
    });
    const t = montar();
    await tick(); await tick(); await tick();
    // abre workflow
    const wfBtn = document.querySelector<HTMLButtonElement>('.wf-card');
    expect(wfBtn).toBeTruthy();
    wfBtn!.click();
    await tick(); await tick(); await tick();
    // abre agente
    const agBtn = document.querySelector<HTMLButtonElement>('.wf-agent');
    expect(agBtn).toBeTruthy();
    agBtn!.click();
    await tick(); await tick(); await tick();
    // prompt renderizado: <strong>Ordem e dependências:</strong> sem **
    const bubble = document.querySelector('.bubble');
    expect(bubble?.innerHTML).toContain('<strong>Ordem e dependências:</strong>');
    expect(bubble?.textContent).not.toContain('**Ordem');
    // resultado renderizado: h4 e strong sem **
    const fala = document.querySelector('.fala');
    expect(fala?.innerHTML).toContain('Bloqueia Task');
    expect(fala?.innerHTML).toContain('<strong>A receita do PATH');
    unmount(t.comp);
  });

  it('mostra total de chamadas (toolCalls) e não a contagem distinta, sem duplicação no rodapé', async () => {
    api.getWorkflows.mockResolvedValue([{ runId: 'wf1', name: 'Test', agentCount: 1, phaseCount: 0, totalTokens: 0, durationMs: 0, startTime: 0, running: false }]);
    api.getWorkflow.mockResolvedValue({
      runId: 'wf1', name: 'Test', status: 'completed', totalTokens: 0, durationMs: 1000, summary: null,
      phases: [], agents: [{ agentId: 'a1', label: 'Agente', phaseTitle: null, state: 'done', model: null, tokens: 0, durationMs: 1000, toolCalls: 3, lastToolName: null, lastToolSummary: null, resultPreview: null }],
    });
    api.getWorkflowAgent.mockResolvedValue({
      agentId: 'a1', label: 'Agente', phaseTitle: null, state: 'done', model: null, tokens: 0, durationMs: 1000, toolCalls: 3,
      prompt: 'p', result: 'r', tools: [{ name: 'Bash', count: 2 }, { name: 'Read', count: 1 }], lastToolName: null, lastToolTarget: null,
    });
    const t = montar();
    await tick(); await tick(); await tick();
    document.querySelector<HTMLButtonElement>('.wf-card')!.click();
    await tick(); await tick(); await tick();
    document.querySelector<HTMLButtonElement>('.wf-agent')!.click();
    await tick(); await tick(); await tick();
    // Ferramentas header deve mostrar 3 chamadas, não 2 (locale-agnostic: apenas o número)
    const ferramentasLabel = document.body.textContent || '';
    // O total deve ser 3, não 2 (distinto)
    expect(ferramentasLabel).toMatch(/3/);
    // Não deve conter "2 chamadas"/"2 calls" como total
    const has2AsTotal = /Ferramentas[^\n]*2 chamadas|Tools[^\n]*2 calls/.test(ferramentasLabel);
    expect(has2AsTotal).toBe(false);
    // Rodapé deve ter apenas uma ocorrência do total (não "3 3 chamadas" / "3 3 calls")
    const fim = document.querySelector('.fim')?.textContent || '';
    // locale-agnostic: conta ocorrências de "3" seguido de chamadas/calls
    const matches = (fim.match(/3/g) || []).length;
    // Deve ter exatamente um "3" (o total), não dois
    expect(matches).toBe(1);
    expect(fim).not.toMatch(/3.*3/);
    unmount(t.comp);
  });

  it('detalhe vivo mostra rodando e última chamada real', async () => {
    api.getWorkflows.mockResolvedValue([{ runId: 'wf1', name: 'Test', agentCount: 1, phaseCount: 0, totalTokens: 0, durationMs: 0, startTime: 0, running: true }]);
    api.getWorkflow.mockResolvedValue({
      runId: 'wf1', name: 'Test', status: 'running', totalTokens: 0, durationMs: 0, summary: null,
      phases: [], agents: [{ agentId: 'a1', label: 'Agente', phaseTitle: null, state: 'progress', model: null, tokens: 0, durationMs: 0, toolCalls: 3, lastToolName: 'Bash', lastToolTarget: 'grep -rn ctx-menu frontend/src', lastToolSummary: null, resultPreview: null }],
    });
    api.getWorkflowAgent.mockResolvedValue({
      agentId: 'a1', label: 'Agente', phaseTitle: null, state: 'progress', model: null, tokens: 0, durationMs: 0, toolCalls: 3,
      prompt: 'prompt vivo', result: null, tools: [{ name: 'Bash', count: 2 }, { name: 'Read', count: 1 }], lastToolName: 'Bash', lastToolTarget: 'grep -rn ctx-menu frontend/src',
    });
    const t = montar();
    await tick(); await tick(); await tick();
    document.querySelector<HTMLButtonElement>('.wf-card')!.click();
    await tick(); await tick(); await tick();
    document.querySelector<HTMLButtonElement>('.wf-agent')!.click();
    await tick(); await tick(); await tick();
    const meta = document.querySelector('.ag-meta')?.textContent || '';
    // locale-agnostic: pt "rodando" ou en "running"
    expect(meta).toMatch(/rodando|running/);
    const agora = document.querySelector('.agora')?.textContent || '';
    expect(agora).toContain('Bash');
    expect(agora).toContain('grep -rn ctx-menu');
    // não deve repetir o nome como alvo falso
    expect(agora).not.toMatch(/Bash — Bash$/);
    unmount(t.comp);
  });
});
