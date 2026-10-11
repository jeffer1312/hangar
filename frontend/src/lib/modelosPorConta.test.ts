import { beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('@hangar/core', async (orig) => ({
  ...(await orig<typeof import('@hangar/core')>()),
  modelOptions: vi.fn(),
  modelOptionsForServer: vi.fn(),
}));

import { modelOptions } from '@hangar/core';
import { carregarModelos } from './modelosPorConta';

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
});
