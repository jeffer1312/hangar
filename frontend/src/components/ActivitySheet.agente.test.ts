// @vitest-environment happy-dom
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { mount, unmount, tick } from 'svelte';
import ActivitySheet from './ActivitySheet.svelte';
import * as apiLib from '@hangar/core';

vi.mock('@hangar/core', async (importOriginal) => ({
  ...(await importOriginal<typeof import('@hangar/core')>()),
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

const SEM_ATIVIDADE = { tasks: [], agents: [], shells: [], inProgress: 0, runningAgents: 0, runningShells: 0 };

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
    api.getWorkflows.mockResolvedValue([{ runId: 'wf1', name: 'Test', status: 'completed', agentCount: 1, phaseCount: 0, totalTokens: 0, durationMs: 0, startTime: 0, running: false }]);
    api.getWorkflow.mockResolvedValue({
      runId: 'wf1', name: 'Test', status: 'completed', totalTokens: 0, durationMs: 0, summary: null,
      phases: [], agents: [{ agentId: 'a1', label: 'Agente', phaseTitle: null, state: 'done', model: 'claude-sonnet', tokens: 100, durationMs: 1000, toolCalls: 3, lastToolName: 'Grep', lastToolSummary: null, resultPreview: null }],
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
    api.getWorkflows.mockResolvedValue([{ runId: 'wf1', name: 'Test', status: 'completed', agentCount: 1, phaseCount: 0, totalTokens: 0, durationMs: 0, startTime: 0, running: false }]);
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
    api.getWorkflows.mockResolvedValue([{ runId: 'wf1', name: 'Test', status: 'running', agentCount: 1, phaseCount: 0, totalTokens: 0, durationMs: 0, startTime: 0, running: true }]);
    api.getWorkflow.mockResolvedValue({
      runId: 'wf1', name: 'Test', status: 'running', totalTokens: 0, durationMs: 0, summary: null,
      phases: [], agents: [{ agentId: 'a1', label: 'Agente', phaseTitle: null, state: 'progress', model: null, tokens: 0, durationMs: 0, toolCalls: 3, lastToolName: 'Bash', lastToolSummary: null, resultPreview: null }],
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
    // bloqueador 1 (r2): com 3 chamadas, NÃO pode mostrar "nenhuma ferramenta chamada"
    expect(document.querySelector('.fala--vazia')).toBeNull();
    expect(document.body.textContent).not.toMatch(/nenhuma ferramenta|no tool/i);
    unmount(t.comp);
  });

  it('detalhe vivo sem ferramentas mostra "Ainda pensando"', async () => {
    api.getWorkflows.mockResolvedValue([{ runId: 'wf1', name: 'Test', status: 'running', agentCount: 1, phaseCount: 0, totalTokens: 0, durationMs: 0, startTime: 0, running: true }]);
    api.getWorkflow.mockResolvedValue({
      runId: 'wf1', name: 'Test', status: 'running', totalTokens: 0, durationMs: 0, summary: null,
      phases: [], agents: [{ agentId: 'a1', label: 'Agente', phaseTitle: null, state: 'progress', model: null, tokens: 0, durationMs: 0, toolCalls: 0, lastToolName: null, lastToolSummary: null, resultPreview: null }],
    });
    api.getWorkflowAgent.mockResolvedValue({
      agentId: 'a1', label: 'Agente', phaseTitle: null, state: 'progress', model: null, tokens: 0, durationMs: 0, toolCalls: 0,
      prompt: 'prompt vivo vazio', result: null, tools: [], lastToolName: null, lastToolTarget: null,
    });
    const t = montar();
    await tick(); await tick(); await tick();
    document.querySelector<HTMLButtonElement>('.wf-card')!.click();
    await tick(); await tick(); await tick();
    document.querySelector<HTMLButtonElement>('.wf-agent')!.click();
    await tick(); await tick(); await tick();
    expect(document.querySelector('.fala--vazia')).not.toBeNull();
    // contém a frase de "pensando" (pt ou en)
    expect(document.body.textContent).toMatch(/Ainda pensando|Still thinking/);
    unmount(t.comp);
  });

  it('prompt longo colapsa com botão mostrar o prompt inteiro e expande ao clicar', async () => {
    // happy-dom não tem layout: scrollHeight é sempre 0 -> precisa mockar para simular overflow
    const longPrompt = 'Você é um revisor **adversarial** de plano de implementação. '.repeat(30)
      + '\n\n1. **Ordem e dependências:** a sequência T1→T5 se sustenta? '.repeat(10);
    api.getWorkflows.mockResolvedValue([{ runId: 'wf1', name: 'Test', status: 'completed', agentCount: 1, phaseCount: 0, totalTokens: 0, durationMs: 0, startTime: 0, running: false }]);
    api.getWorkflow.mockResolvedValue({
      runId: 'wf1', name: 'Test', status: 'completed', totalTokens: 0, durationMs: 0, summary: null,
      phases: [], agents: [{ agentId: 'a1', label: 'Agente', phaseTitle: null, state: 'done', model: 'claude-sonnet', tokens: 100, durationMs: 1000, toolCalls: 3, lastToolName: 'Grep', lastToolSummary: null, resultPreview: null }],
    });
    api.getWorkflowAgent.mockResolvedValue({
      agentId: 'a1', label: 'Agente', phaseTitle: null, state: 'done', model: 'claude-sonnet', tokens: 100, durationMs: 1000, toolCalls: 3,
      prompt: longPrompt, result: 'resultado ok', tools: [{ name: 'Bash', count: 1 }], lastToolName: 'Bash', lastToolTarget: 'test',
    });
    // força overflow: bubble com scrollHeight 400 (>136)
    const spy = vi.spyOn(HTMLElement.prototype, 'scrollHeight', 'get').mockReturnValue(400);
    const t = montar();
    await tick(); await tick(); await tick();
    document.querySelector<HTMLButtonElement>('.wf-card')!.click();
    await tick(); await tick(); await tick();
    document.querySelector<HTMLButtonElement>('.wf-agent')!.click();
    await tick(); await tick(); await tick();
    // o check roda via tick().then(check) + requestAnimationFrame: espera os dois
    await tick();
    await new Promise<void>((r) => requestAnimationFrame(() => r()));
    await tick(); await tick();
    const bubble = document.querySelector('.bubble') as HTMLElement | null;
    expect(bubble).not.toBeNull();
    expect(bubble!.classList.contains('clamp')).toBe(true);
    const btn = document.querySelector<HTMLButtonElement>('.ver-tudo');
    expect(btn).not.toBeNull();
    expect(btn!.textContent).toMatch(/mostrar o prompt inteiro|show full prompt/i);
    // clicar expande
    btn!.click();
    await tick(); await tick();
    expect(bubble!.classList.contains('clamp')).toBe(false);
    expect(document.querySelector('.ver-tudo')).toBeNull();
    spy.mockRestore();
    unmount(t.comp);
  });

  it('prompt curto não mostra botão de expandir', async () => {
    const shortPrompt = 'prompt curto';
    api.getWorkflows.mockResolvedValue([{ runId: 'wf1', name: 'Test', status: 'completed', agentCount: 1, phaseCount: 0, totalTokens: 0, durationMs: 0, startTime: 0, running: false }]);
    api.getWorkflow.mockResolvedValue({
      runId: 'wf1', name: 'Test', status: 'completed', totalTokens: 0, durationMs: 0, summary: null,
      phases: [], agents: [{ agentId: 'a1', label: 'Agente', phaseTitle: null, state: 'done', model: null, tokens: 0, durationMs: 1000, toolCalls: 0, lastToolName: null, lastToolSummary: null, resultPreview: null }],
    });
    api.getWorkflowAgent.mockResolvedValue({
      agentId: 'a1', label: 'Agente', phaseTitle: null, state: 'done', model: null, tokens: 0, durationMs: 1000, toolCalls: 0,
      prompt: shortPrompt, result: 'r', tools: [], lastToolName: null, lastToolTarget: null,
    });
    const spy = vi.spyOn(HTMLElement.prototype, 'scrollHeight', 'get').mockReturnValue(20);
    const t = montar();
    await tick(); await tick(); await tick();
    document.querySelector<HTMLButtonElement>('.wf-card')!.click();
    await tick(); await tick(); await tick();
    document.querySelector<HTMLButtonElement>('.wf-agent')!.click();
    await tick(); await tick(); await tick();
    await tick();
    await new Promise<void>((r) => requestAnimationFrame(() => r()));
    await tick();
    expect(document.querySelector('.ver-tudo')).toBeNull();
    expect(document.querySelector('.bubble')?.classList.contains('clamp')).toBe(false);
    spy.mockRestore();
    unmount(t.comp);
  });

  it('prompt longo responde a ResizeObserver em troca de largura 1280→390→1280', async () => {
    const longPrompt = 'linha longa '.repeat(80);
    api.getWorkflows.mockResolvedValue([{ runId: 'wf1', name: 'Test', status: 'completed', agentCount: 1, phaseCount: 0, totalTokens: 0, durationMs: 0, startTime: 0, running: false }]);
    api.getWorkflow.mockResolvedValue({
      runId: 'wf1', name: 'Test', status: 'completed', totalTokens: 0, durationMs: 0, summary: null,
      phases: [], agents: [{ agentId: 'a1', label: 'Agente', phaseTitle: null, state: 'done', model: null, tokens: 0, durationMs: 1000, toolCalls: 0, lastToolName: null, lastToolSummary: null, resultPreview: null }],
    });
    api.getWorkflowAgent.mockResolvedValue({
      agentId: 'a1', label: 'Agente', phaseTitle: null, state: 'done', model: null, tokens: 0, durationMs: 1000, toolCalls: 0,
      prompt: longPrompt, result: 'r', tools: [], lastToolName: null, lastToolTarget: null,
    });
    // captura callback do ResizeObserver para simular resize
    let roCb: ResizeObserverCallback | null = null;
    const RealRO = globalThis.ResizeObserver;
    class MockRO {
      constructor(cb: ResizeObserverCallback) { roCb = cb; }
      observe(_target: Element) {}
      disconnect() {}
      unobserve(_target: Element) {}
    }
    (globalThis as unknown as { ResizeObserver: typeof MockRO }).ResizeObserver = MockRO as unknown as typeof RealRO;
    const spy = vi.spyOn(HTMLElement.prototype, 'scrollHeight', 'get').mockReturnValue(300);
    const t = montar();
    await tick(); await tick(); await tick();
    document.querySelector<HTMLButtonElement>('.wf-card')!.click();
    await tick(); await tick(); await tick();
    document.querySelector<HTMLButtonElement>('.wf-agent')!.click();
    await tick(); await tick(); await tick();
    await tick();
    await new Promise<void>((r) => requestAnimationFrame(() => r()));
    await tick();
    expect(document.querySelector('.bubble')?.classList.contains('clamp')).toBe(true);
    expect(document.querySelector('.ver-tudo')).not.toBeNull();
    // simula resize estreito -> ainda overflow (mantém clamp)
    if (roCb) (roCb as unknown as () => void)();
    await tick();
    expect(document.querySelector('.bubble')?.classList.contains('clamp')).toBe(true);
    // volta largo -> ainda overflow (prompt continua longo)
    if (roCb) (roCb as unknown as () => void)();
    await tick();
    expect(document.querySelector('.bubble')?.classList.contains('clamp')).toBe(true);
    spy.mockRestore();
    (globalThis as unknown as { ResizeObserver: typeof RealRO }).ResizeObserver = RealRO;
    unmount(t.comp);
  });
});
