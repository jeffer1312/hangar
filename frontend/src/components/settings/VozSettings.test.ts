// @vitest-environment happy-dom
import { describe, it, expect, vi } from 'vitest';
import { mount, unmount } from 'svelte';
import VozSettings from './VozSettings.svelte';
import * as m from '../../paraglide/messages';
import type { ConfigServidorStore } from '../../lib/serverConfig.svelte';

vi.mock('@hangar/core', async (importOriginal) => ({
  ...(await importOriginal<typeof import('@hangar/core')>()), listarVozesTts: vi.fn(async () => []), saldoTts: vi.fn(async () => ({ usados: 0, limite: 0 })), getConfig: vi.fn(async () => ({ campos: {}, somente_leitura: {} })) }));
vi.mock('../../lib/ttsPlayer.svelte', () => ({ ttsPlayer: { tocando: false, parar: vi.fn() } }));
vi.mock('../../lib/ouvir', () => ({ ouvirAmostra: vi.fn() }));

function montar(campos: Record<string, unknown>) {
  const alvo = document.createElement('div');
  document.body.appendChild(alvo);
  const store = {
    get campos() { return campos; }, get leitura() { return {}; },
    get carregando() { return false; }, get salvando() { return false; },
    get erro() { return ''; }, get salvo() { return false; }, get temMudanca() { return false; },
    valorAtual: () => '', rascunhoDe: () => '', setRascunho: vi.fn(),
    carregar: vi.fn(), salvar: vi.fn(), invalidar: vi.fn(),
  } as unknown as ConfigServidorStore;
  const app = mount(VozSettings, { target: alvo, props: { store } });
  return { alvo, app };
}

describe('VozSettings', () => {
  it('sem chave de transcrição, avisa que ditar está desligado', () => {
    const { alvo, app } = montar({ groq_api_key: { definido: false } });
    expect(alvo.textContent).toContain(m.voz_transcrever_sem_chave());
    unmount(app);
  });

  it('o avançado do LLM nasce fechado', () => {
    const { alvo, app } = montar({ groq_api_key: { definido: true } });
    expect(alvo.textContent).not.toContain(m.config_server_endpoint_llm());
    expect(alvo.textContent).toContain(m.voz_usar_outro_servico());
    unmount(app);
  });

  it('sem chave de voz, a leitura em voz alta aparece desligada, não some', () => {
    const { alvo, app } = montar({ elevenlabs_api_key: { definido: false } });
    expect(alvo.textContent).toContain(m.voz_ler_sem_chave());
    unmount(app);
  });
});
