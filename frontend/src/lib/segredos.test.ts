// @vitest-environment happy-dom
import { describe, it, expect, vi, beforeEach } from 'vitest';

const getConfig = vi.fn();
// Sem importOriginal de propósito: o pacote inteiro arrasta as 2400 mensagens do paraglide e a
// carga a frio dentro do teste (resetModules + import dinâmico) estoura os 5s.
vi.mock('@hangar/core', () => ({ getConfig: (...a: unknown[]) => getConfig(...a) }));

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

  it('podeLer: chave da ElevenLabs sozinha já basta', async () => {
    getConfig.mockResolvedValue({ campos: { elevenlabs_api_key: { definido: true }, tts_local_cmd: { valor: '' } }, somente_leitura: {} });
    const { segredos } = await import('./segredos.svelte');
    await segredos.carregar();
    expect(segredos.podeLer()).toBe(true);
  });

  it('podeLer: sem chave, comando local não-vazio também basta', async () => {
    getConfig.mockResolvedValue({ campos: { elevenlabs_api_key: { definido: false }, tts_local_cmd: { valor: 'echo oi' } }, somente_leitura: {} });
    const { segredos } = await import('./segredos.svelte');
    await segredos.carregar();
    expect(segredos.podeLer()).toBe(true);
  });

  it('podeLer: sem chave e comando local vazio ou só espaço, não dá pra ler', async () => {
    getConfig.mockResolvedValue({ campos: { elevenlabs_api_key: { definido: false }, tts_local_cmd: { valor: '   ' } }, somente_leitura: {} });
    const { segredos } = await import('./segredos.svelte');
    await segredos.carregar();
    expect(segredos.podeLer()).toBe(false);
  });

  it('resposta em voo de uma troca velha não sobrescreve a troca mais nova', async () => {
    // Servidor A (chave ausente) demora a responder; servidor B (chave presente) responde na hora.
    // A troca rápida A -> B não pode deixar a resposta atrasada de A vencer por chegar depois.
    let resolveA!: (v: unknown) => void;
    getConfig.mockImplementationOnce(() => new Promise((r) => { resolveA = r; }));
    const { segredos } = await import('./segredos.svelte');

    const carregandoA = segredos.carregar(); // troca pro servidor A — fica pendente

    getConfig.mockResolvedValueOnce({ campos: { elevenlabs_api_key: { definido: true } }, somente_leitura: {} });
    await segredos.carregar(); // troca pro servidor B — resolve na hora
    expect(segredos.temChave('elevenlabs_api_key')).toBe(true);

    resolveA({ campos: { elevenlabs_api_key: { definido: false } }, somente_leitura: {} });
    await carregandoA; // resposta atrasada de A chega agora

    expect(segredos.temChave('elevenlabs_api_key')).toBe(true); // continua valendo B, a mais nova
  });
});
