// @vitest-environment happy-dom
import { describe, it, expect, vi, beforeEach } from 'vitest';

const getConfig = vi.fn();
vi.mock('./api', () => ({ getConfig: (...a: unknown[]) => getConfig(...a) }));

describe('segredos', () => {
  beforeEach(() => { vi.resetModules(); getConfig.mockReset(); });

  it('antes de carregar, ninguém tem chave', async () => {
    const { segredos } = await import('./segredos.svelte');
    expect(segredos.temChave('elevenlabs_api_key')).toBe(false);
  });

  it('depois de carregar, reflete o definido do backend', async () => {
    getConfig.mockResolvedValue({ campos: { elevenlabs_api_key: { definido: true }, groq_api_key: { definido: false } }, somente_leitura: {} });
    const { segredos } = await import('./segredos.svelte');
    await segredos.carregar();
    expect(segredos.temChave('elevenlabs_api_key')).toBe(true);
    expect(segredos.temChave('groq_api_key')).toBe(false);
  });

  it('falha de rede não derruba: segue sem chave e não relança', async () => {
    getConfig.mockRejectedValue(new Error('sem rede'));
    const { segredos } = await import('./segredos.svelte');
    await expect(segredos.carregar()).resolves.toBeUndefined();
    expect(segredos.temChave('elevenlabs_api_key')).toBe(false);
  });
});
