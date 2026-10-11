// @vitest-environment happy-dom
import { beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('@hangar/core', async (orig) => ({
  ...(await orig<typeof import('@hangar/core')>()),
  modelOptions: vi.fn(),
  modelOptionsForServer: vi.fn(),
}));

import { modelOptions } from '@hangar/core';
import { carregarModelos, matchRemembered } from './modelosPorConta';

describe('carregarModelos: lembrado com 1M', () => {
  beforeEach(() => localStorage.clear());

  it('devolve o modelo base e o 1M separado, só com conta ChatGPT e modelo com Fast', async () => {
    vi.mocked(modelOptions).mockResolvedValue(
      { models: [{ id: 'gpt-5.5', supports_fast: true }, { id: 'gpt-4o' }], reduced: false } as never,
    );
    localStorage.setItem('k', 'gpt-5.5[1m]');
    expect(await carregarModelos({ provider: 'claude', engine: 'gpt', engineAccount: 'a' }, 'k'))
      .toMatchObject({ lembrado: 'gpt-5.5', contextoLembrado: true });
    expect(await carregarModelos({ provider: 'claude', engine: 'gpt' }, 'k'))
      .toMatchObject({ lembrado: 'gpt-5.5', contextoLembrado: false });
    localStorage.setItem('k', 'gpt-4o[1m]');
    expect(await carregarModelos({ provider: 'claude', engine: 'gpt', engineAccount: 'a' }, 'k'))
      .toMatchObject({ lembrado: 'gpt-4o', contextoLembrado: false });
  });

  it('conta Anthropic: opus[1m] é linha própria e volta inteiro', async () => {
    vi.mocked(modelOptions).mockResolvedValue({ models: [{ id: 'opus' }, { id: 'opus[1m]' }], reduced: false } as never);
    localStorage.setItem('k', 'opus[1m]');
    expect(await carregarModelos({ provider: 'claude' }, 'k'))
      .toMatchObject({ lembrado: 'opus[1m]', contextoLembrado: false });
    // Lista fria sem a linha de 1M: não troca calado pelo Opus comum.
    vi.mocked(modelOptions).mockResolvedValue({ models: [{ id: 'opus' }], reduced: true } as never);
    expect(await carregarModelos({ provider: 'claude' }, 'k')).toMatchObject({ lembrado: '' });
  });
});

describe('matchRemembered', () => {
  const models = [{ id: 'opus' }, { id: 'opus[1m]' }, { id: 'gpt-5.5', supports_fast: true }] as never;
  it('id exato vence; base sem [1m] só no motor, 1M só na conta do proxy', () => {
    expect(matchRemembered('opus[1m]', models, false, false)).toEqual({ model: 'opus[1m]', context: false });
    expect(matchRemembered('gpt-5.5[1m]', models, false, false)).toBeNull();
    expect(matchRemembered('gpt-5.5[1m]', models, true, false)).toEqual({ model: 'gpt-5.5', context: false });
    expect(matchRemembered('gpt-5.5[1m]', models, true, true)).toEqual({ model: 'gpt-5.5', context: true });
  });
});
