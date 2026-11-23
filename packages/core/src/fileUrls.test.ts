import { describe, it, expect, beforeEach, vi } from 'vitest';
import { configureApi } from './apiEnv';
import { fileUrl, fileUrlNative, fileAuthHeader, uploadUrl, uploadUrlNative, transcriptImageUrlNative, sintetizarTts, ttsAudioUrl } from './api';
import type { Server } from './servers';

function stubEventSource() {
  return { addEventListener() {}, removeEventListener() {}, close() {}, onerror: null, onopen: null, readyState: 0 } as unknown as import('./apiEnv').EventSourceLike;
}

describe('fileUrls token handling', () => {
  beforeEach(() => {
    vi.restoreAllMocks();
    configureApi({
      getBaseUrl: () => 'https://backend.example',
      getToken: () => 'CP-token-real-do-usuario',
      onUnauthorized: () => {},
      origin: 'https://app.test',
      createEventSource: () => stubEventSource(),
    });
  });

  it('fileUrl (browser) contém token na query', () => {
    const url = fileUrl('fixture-t12', 'relatorio.html');
    expect(url).toContain('token=CP-token-real-do-usuario');
  });

  it('fileUrlNative não contém token e header tem Bearer', () => {
    const url = fileUrlNative('fixture-t12', 'relatorio.html');
    expect(url).not.toContain('token=');
    expect(url).toContain('/file?path=');
    const h = fileAuthHeader();
    expect(h).toEqual({ Authorization: 'Bearer CP-token-real-do-usuario' });
  });

  it('uploadUrl vs uploadUrlNative seguem mesmo contrato', () => {
    expect(uploadUrl('s', 'a.png')).toContain('token=');
    expect(uploadUrlNative('s', 'a.png')).not.toContain('token=');
  });

  it('mídia nativa usa endereço e credencial da conversa, não os do ativo', () => {
    const server: Server = { id: 'a', label: 'A', baseUrl: 'https://a.test', token: 'token-a' };
    expect(fileUrlNative('s 1', '/tmp/a.png', server)).toBe('https://a.test/api/sessions/s%201/file?path=%2Ftmp%2Fa.png');
    expect(uploadUrlNative('s 1', 'a b.png', server)).toBe('https://a.test/api/sessions/s%201/uploads/a%20b.png');
    expect(transcriptImageUrlNative('s 1', 'msg/1', 2, server)).toBe('https://a.test/api/sessions/s%201/transcript-image/msg%2F1/2');
    expect(fileAuthHeader(server)).toEqual({ Authorization: 'Bearer token-a' });
    expect(transcriptImageUrlNative('s', 'msg', 0)).toBe('https://backend.example/api/sessions/s/transcript-image/msg/0');
  });

  it('miniatura nativa pede a largura ao servidor sem levar o token na URL', () => {
    const server: Server = { id: 'a', label: 'A', baseUrl: 'https://a.test', token: 'token-a' };
    expect(fileUrlNative('s', '/tmp/a.png', server, 288)).toBe('https://a.test/api/sessions/s/file?path=%2Ftmp%2Fa.png&w=288');
    expect(uploadUrlNative('s', 'a.png', server, 288)).toBe('https://a.test/api/sessions/s/uploads/a.png?w=288');
    expect(uploadUrlNative('s', 'a.png', server, 288)).not.toContain('token=');
    expect(fileUrlNative('s', '/tmp/a.png', server)).not.toContain('w=');
  });

  it('TTS fixa POST e áudio em A mesmo com outro servidor ativo durante a espera', async () => {
    const server: Server = { id: 'a', label: 'A', baseUrl: 'https://a.test', token: 'token-a' };
    let active = { baseUrl: 'https://b.test', token: 'token-b' };
    configureApi({
      getBaseUrl: () => active.baseUrl, getToken: () => active.token, onUnauthorized: () => {},
      origin: null, createEventSource: () => stubEventSource(),
    });
    let finish!: (r: Response) => void;
    const request = vi.spyOn(globalThis, 'fetch').mockImplementationOnce(() => new Promise((resolve) => { finish = resolve; }));
    const pending = sintetizarTts({ text: 'resposta de A' }, server);
    expect(request).toHaveBeenCalledWith('https://a.test/api/tts', expect.objectContaining({
      method: 'POST', body: JSON.stringify({ text: 'resposta de A' }),
      headers: expect.objectContaining({ Authorization: 'Bearer token-a' }),
    }));
    active = { baseUrl: 'https://c.test', token: 'token-c' };
    finish(Response.json({ url: '/api/tts/audio/1.mp3', chars: 13, cached: false, provider: 'local' }));
    const result = await pending;
    expect(ttsAudioUrl(result.url, server)).toBe('https://a.test/api/tts/audio/1.mp3?token=token-a');
    expect(ttsAudioUrl(result.url)).toBe('https://c.test/api/tts/audio/1.mp3?token=token-c');
  });

  it('TTS sem servidor explícito preserva o ativo da PWA', async () => {
    const request = vi.spyOn(globalThis, 'fetch').mockResolvedValueOnce(Response.json({
      url: '/api/tts/audio/1.mp3', chars: 5, cached: false, provider: 'local',
    }));
    await sintetizarTts({ text: 'texto' });
    expect(request).toHaveBeenCalledWith('https://backend.example/api/tts', expect.objectContaining({
      method: 'POST', headers: expect.objectContaining({ Authorization: 'Bearer CP-token-real-do-usuario' }),
    }));
    expect(ttsAudioUrl('/api/tts/audio/1.mp3')).toBe('https://backend.example/api/tts/audio/1.mp3?token=CP-token-real-do-usuario');
  });

  it('mutação: se fileUrlNative voltasse a anexar token, este teste ficaria vermelho', () => {
    const url = fileUrlNative('s', 'x.html');
    // esta asserção é a que falharia se alguém reintroduzisse &token= no nativo
    expect(url).not.toMatch(/token=/);
  });
});
