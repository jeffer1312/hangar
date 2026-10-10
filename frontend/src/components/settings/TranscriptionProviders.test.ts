// @vitest-environment happy-dom
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { mount, unmount, tick } from 'svelte';
import { getTranscriptionProvidersStatus } from '@hangar/core';
import TranscriptionProviders from './TranscriptionProviders.svelte';
import * as m from '../../paraglide/messages';
import type { ConfigServidorStore } from '../../lib/serverConfig.svelte';
import { overwriteGetLocale } from '../../paraglide/runtime';

beforeEach(() => overwriteGetLocale(() => 'pt'));

vi.mock('@hangar/core', async (orig) => ({
  ...(await orig<typeof import('@hangar/core')>()),
  getTranscriptionProvidersStatus: vi.fn(async () => ({ providers: [] })),
}));

const ELEVEN = { id: 'a', kind: 'elevenlabs', name: '', base_url: '', api_key: 'xi_••••', model: '' };
const GROQ = { id: 'b', kind: 'openai', name: '', base_url: 'https://api.groq.com/openai/v1', api_key: 'gsk_••••', model: 'whisper-large-v3-turbo' };
const flush = async () => { await tick(); await new Promise((r) => setTimeout(r, 0)); await tick(); };

function montar(valor: unknown[]) {
  const alvo = document.createElement('div');
  document.body.appendChild(alvo);
  const setRascunho = vi.fn();
  const campos = { transcription_providers: { valor, definido: true, origem: 'app' } };
  const store = {
    get campos() { return campos; }, get carregando() { return false; }, get alvo() { return null; },
    valorBruto: () => valor, setRascunho,
  } as unknown as ConfigServidorStore;
  const app = mount(TranscriptionProviders, { target: alvo, props: { store } });
  return { alvo, app, setRascunho };
}

describe('TranscriptionProviders', () => {
  it('Whisper local pede arquivos do servidor e não pede chave de API', () => {
    const local = { id: 'local', kind: 'whisper_cpp', name: '', base_url: '', api_key: '', model: '',
      executable_path: '/opt/Whisper instalado/whisper-server', model_path: '/opt/Whisper instalado/model.bin',
      language: 'pt', converter_path: '' };
    const { alvo, app } = montar([local]);
    expect(alvo.querySelector<HTMLInputElement>('input[name="executable_path"]')?.value).toBe(local.executable_path);
    expect(alvo.querySelector<HTMLInputElement>('input[name="model_path"]')?.value).toBe(local.model_path);
    expect(alvo.querySelector('input[type="password"]')).toBeNull();
    expect(alvo.querySelector<HTMLAnchorElement>('a[href="https://github.com/ggml-org/whisper.cpp"]')).not.toBeNull();
    unmount(app);
  });
  it('vazia: diz que vale o serviço único', () => {
    const { alvo, app } = montar([]);
    expect(alvo.textContent).toContain(m.native_voice_providers_empty());
    unmount(app);
  });

  it('nome do serviço só no item, na ordem; o primeiro não sobe', () => {
    const { alvo, app } = montar([ELEVEN, GROQ]);
    const nomes = [...alvo.querySelectorAll('.nome')].map((n) => n.textContent);
    expect(nomes).toEqual(['ElevenLabs', 'api.groq.com · whisper-large-v3-turbo']);
    expect(alvo.querySelector<HTMLButtonElement>(`button[aria-label="${m.native_voice_provider_up({ name: 'ElevenLabs' })}"]`)!.disabled).toBe(true);
    unmount(app);
  });

  it('endpoint só no tipo compatível com OpenAI', () => {
    const { alvo, app } = montar([ELEVEN, GROQ]);
    expect(alvo.querySelectorAll('input[type="url"]')).toHaveLength(1);
    unmount(app);
  });

  it('descer troca a ordem; adicionar acrescenta no fim, sem chave', () => {
    const { alvo, app, setRascunho } = montar([ELEVEN, GROQ]);
    alvo.querySelector<HTMLButtonElement>(`button[aria-label="${m.native_voice_provider_down({ name: 'ElevenLabs' })}"]`)!.click();
    expect(setRascunho).toHaveBeenLastCalledWith('transcription_providers', [GROQ, ELEVEN]);
    [...alvo.querySelectorAll('button')].find((b) => b.textContent === m.native_voice_provider_add())!.click();
    const ultima = setRascunho.mock.calls.at(-1)![1] as { kind: string; id: string }[];
    expect(ultima).toHaveLength(3);
    expect(ultima[2]).toMatchObject({ kind: 'openai', api_key: '' });
    unmount(app);
  });

  it('apagar a chave digitada devolve a máscara (o servidor mantém a guardada)', () => {
    const { alvo, app, setRascunho } = montar([ELEVEN]);
    const chave = alvo.querySelector<HTMLInputElement>('#tp-key-a')!;
    expect(chave.value).toBe('');
    expect(chave.type).toBe('password');
    chave.value = 'nova';
    // O Svelte 5 delega `input` na raiz da montagem: sem bolhar, o handler não roda.
    chave.dispatchEvent(new Event('input', { bubbles: true }));
    expect(setRascunho).toHaveBeenLastCalledWith('transcription_providers', [{ ...ELEVEN, api_key: 'nova' }]);
    chave.value = '';
    chave.dispatchEvent(new Event('input', { bubbles: true }));
    expect(setRascunho).toHaveBeenLastCalledWith('transcription_providers', [ELEVEN]);
    unmount(app);
  });

  it('trocar o tipo de item salvo pede a chave de novo; tocar no tipo atual não mexe no rascunho', () => {
    const { alvo, app, setRascunho } = montar([GROQ]);
    const opcao = (rotulo: string) => [...alvo.querySelectorAll<HTMLButtonElement>('button')]
      .find((b) => b.textContent?.trim() === rotulo)!;
    opcao(m.native_voice_provider_kind_openai()).click();
    expect(setRascunho).not.toHaveBeenCalled();
    opcao(m.native_voice_provider_kind_elevenlabs()).click();
    expect(setRascunho).toHaveBeenLastCalledWith('transcription_providers', [{ ...GROQ, kind: 'elevenlabs', api_key: '' }]);
    unmount(app);
  });

  it('somente ElevenLabs sem chave avisa que falta a chave', () => {
    const { alvo, app } = montar([{ ...ELEVEN, api_key: '' }, { ...GROQ, api_key: '' }]);
    expect(alvo.querySelectorAll('.falta')).toHaveLength(1);
    expect(alvo.textContent).toContain(m.native_voice_provider_missing_key());
    unmount(app);
  });

  it('serviço sem cota mostra até quando espera', async () => {
    vi.mocked(getTranscriptionProvidersStatus).mockResolvedValueOnce({ providers: [
      { id: 'a', name: 'ElevenLabs', kind: 'elevenlabs', waiting_until: Date.now() / 1000 + 3600, reason: 'quota' },
    ] });
    const { alvo, app } = montar([ELEVEN]);
    await flush();
    expect(alvo.querySelector('.espera')?.textContent).toContain('quota');
    unmount(app);
  });

  it('falha ao ler o estado aparece, não some', async () => {
    vi.mocked(getTranscriptionProvidersStatus).mockRejectedValueOnce(new Error('404'));
    const { alvo, app } = montar([ELEVEN]);
    await flush();
    expect(alvo.textContent).toContain(m.native_voice_providers_status_failed({ error: '404' }));
    unmount(app);
  });
});
