import { beforeEach, describe, expect, it, vi } from 'vitest';

const native = vi.hoisted(() => ({
  upload: vi.fn(),
  uris: [] as string[],
}));

vi.mock('expo-file-system', () => ({
  File: class {
    constructor(uri: string) { native.uris.push(uri); }
    upload(url: string, options: unknown) { return native.upload(url, options); }
  },
}));

import { sendFromDisk } from './uploadFromDisk';

const URL_UPLOAD = 'http://maquina:8765/api/sessions/s/upload';

describe('sendFromDisk', () => {
  beforeEach(() => {
    native.upload.mockReset();
    native.uris = [];
  });

  it('sobe o arquivo do disco com os cabeçalhos do pedido e devolve a resposta do servidor', async () => {
    native.upload.mockResolvedValue({ status: 200, body: '{"path":"/up/pacote.zip"}', headers: { 'content-type': 'application/json' } });
    const headers = { 'Content-Type': 'application/zip', 'X-Filename': 'pacote.zip', Authorization: 'Bearer t' };
    const signal = new AbortController().signal;
    const res = await sendFromDisk('file:///docs/pacote.zip')(URL_UPLOAD, { method: 'POST', headers, signal });
    expect(native.uris).toEqual(['file:///docs/pacote.zip']);
    expect(native.upload).toHaveBeenCalledExactlyOnceWith(URL_UPLOAD, { httpMethod: 'POST', headers, sessionType: 'foreground', signal });
    expect(res.status).toBe(200);
    expect(await res.json()).toEqual({ path: '/up/pacote.zip' });
  });

  it('erro HTTP do servidor chega como resposta, não como queda de rede', async () => {
    native.upload.mockResolvedValue({ status: 413, body: '{"detail":"grande demais"}', headers: {} });
    const res = await sendFromDisk('file:///docs/pacote.zip')(URL_UPLOAD, { method: 'POST', headers: {} });
    expect(res.ok).toBe(false);
    expect(res.status).toBe(413);
  });

  it('prazo estourado rejeita com o motivo do sinal, não com o AbortError do envio nativo', async () => {
    const controller = new AbortController();
    const reason = new DOMException('prazo', 'TimeoutError');
    native.upload.mockImplementation(async () => {
      controller.abort(reason);
      throw new DOMException('Aborted', 'AbortError');
    });
    const reply = sendFromDisk('file:///docs/pacote.zip')(URL_UPLOAD, { method: 'POST', headers: {}, signal: controller.signal });
    await expect(reply).rejects.toBe(reason);
  });

  it('falha de rede do envio nativo chega como está', async () => {
    const failure = new Error('The network connection was lost.');
    native.upload.mockRejectedValue(failure);
    await expect(sendFromDisk('file:///docs/pacote.zip')(URL_UPLOAD, { method: 'POST', headers: {} })).rejects.toBe(failure);
  });
});
