// @vitest-environment happy-dom
import { expect, it, vi } from 'vitest';
import { createRawSnippet, mount, tick, unmount } from 'svelte';
import type { CostReport } from '@hangar/core';
import { clienteQuery } from '../lib/queries';
import * as m from '../paraglide/messages';
import Costs from './Costs.svelte';

vi.mock('../components/NavBar.svelte', () => ({ default: createRawSnippet(() => ({ render: () => '<nav></nav>' })) }));
vi.mock('../lib/queries', () => ({
  custos: (server: { id: string }, period: string) => ({ id: server.id, period }),
  clienteQuery: { fetchQuery: vi.fn(), invalidateQueries: vi.fn(async () => {}) },
}));

it('mostra a máquina rápida, alterna tokens/custo e preserva o período quando a lenta responde', async () => {
  localStorage.clear();
  localStorage.setItem('cp_servers', JSON.stringify(['fast', 'slow'].map((id) => ({
    id, label: id, baseUrl: `https://${id}.test`, token: 'test',
  }))));
  const bucket = { key: 'totals', sessions: 1, input: 100, output: 20, cache_read: 80, cache_write: 0,
    cost: 2, cost_input: 1, cost_output: 0.8, cost_cache_read: 0.2, cost_cache_write: 0 };
  const report = (period: string): Partial<CostReport> => ({
    totals: bucket, applied: { period },
    by_day: [{ ...bucket, key: '2026-09-10' }],
    by_source: [{ ...bucket, key: 'codex' }],
    by_provider: [{ ...bucket, key: 'openai' }],
    combos: [{ ...bucket, dia: '2026-09-10', source: 'codex', provider: 'openai',
      model: 'test', project: '/test', subagente: false }],
  });
  let finishSlow!: (value: Partial<CostReport>) => void;
  const slow = new Promise<Partial<CostReport>>((resolve) => { finishSlow = resolve; });
  vi.mocked(clienteQuery.fetchQuery).mockImplementation((query) => {
    const { id, period } = query as unknown as { id: string; period: string };
    return (id === 'slow' ? slow : Promise.resolve(report(period))) as ReturnType<typeof clienteQuery.fetchQuery>;
  });
  const target = document.body.appendChild(document.createElement('div'));
  const component = mount(Costs, { target, props: { onBack: vi.fn() } });
  const settle = async () => { for (let i = 0; i < 12; i++) await tick(); };
  const button = (label: string) => [...target.querySelectorAll('button')].find((b) => b.textContent?.trim() === label)!;
  try {
    await settle();
    expect(target.querySelector('.overview')).not.toBeNull();
    expect(target.textContent).toContain(m.custos_carregando_maquinas({ n: 1 }));
    expect(target.querySelector('svg')?.getAttribute('aria-label')).toContain(m.ctx_tokens());
    button(m.custos_estimativa_api()).click();
    await settle();
    expect(target.querySelector('svg')?.getAttribute('aria-label')).toContain(m.custos_estimativa_api());
    button(m.custos_periodo_7d()).click();
    await settle();
    finishSlow(report('30d'));
    await settle();
    expect(button(m.custos_periodo_7d()).getAttribute('aria-pressed')).toBe('true');
    expect(target.querySelector('.loading-status')).toBeNull();
    expect(target.textContent).toContain(m.custos_fora_periodo_1());
    expect(target.querySelector('.overview')?.textContent).toContain('200');
  } finally { await unmount(component); target.remove(); localStorage.clear(); }
});

it('distingue contas de mesmo nome no Claude e Codex ao comparar', async () => {
  localStorage.clear();
  localStorage.setItem('cp_servers', JSON.stringify([{ id: 'local', label: 'Local', baseUrl: 'https://local.test', token: 'test' }]));
  const bucket = { key: 'totals', sessions: 1, input: 100, output: 20, cache_read: 80, cache_write: 0,
    cost: 2, cost_input: 1, cost_output: 0.8, cost_cache_read: 0.2, cost_cache_write: 0 };
  const accounts = [
    { key: 'anthropic:account', source: 'claude', label: 'claude-200-2' },
    { key: 'codex:/accounts/work', source: 'codex', label: 'claude-200-2' },
  ];
  vi.mocked(clienteQuery.fetchQuery).mockResolvedValue({
    totals: { ...bucket, sessions: 2, cost: 4 }, applied: { period: '30d' },
    by_provider: accounts.map((account) => ({ ...bucket, key: account.key, label: account.label })),
    by_source: accounts.map((account) => ({ ...bucket, key: account.source })),
    combos: accounts.map((account) => ({ ...bucket, dia: '2026-09-10', provider: account.key,
      source: account.source, model: 'test', project: '/test', subagente: false })),
  });
  const target = document.body.appendChild(document.createElement('div'));
  const component = mount(Costs, { target, props: { onBack: vi.fn() } });
  try {
    for (let i = 0; i < 12; i++) await tick();
    const choices = target.querySelector(`[aria-label="${m.custos_entidades_comparadas()}"]`)!;
    const buttons = [...choices.querySelectorAll('button')];
    expect(buttons.map((button) => button.textContent?.trim())).toEqual([
      `${m.custos_claude_code()} · claude-200-2`, 'Codex · claude-200-2',
    ]);
    for (const button of buttons) button.click();
    await tick();
    const cards = [...target.querySelectorAll('.cmpnome')].map((card) => card.textContent?.trim());
    expect(cards).toEqual(buttons.map((button) => button.textContent?.trim()));
    expect(target.querySelectorAll('.cmpvalor')).toHaveLength(2);
  } finally { await unmount(component); target.remove(); localStorage.clear(); }
});
