import { beforeEach, describe, expect, it, vi } from 'vitest';

const native = vi.hoisted(() => ({
  createUploadTask: vi.fn(),
  cancel: vi.fn(async () => {}),
  finish: undefined as undefined | ((result: { status: number; body: string; headers: Record<string, string> }) => void),
  fail: undefined as undefined | ((error: Error) => void),
}));

vi.mock('expo-file-system/legacy', () => ({
  FileSystemUploadType: { BINARY_CONTENT: 0, MULTIPART: 1 },
  FileSystemSessionType: { BACKGROUND: 0, FOREGROUND: 1 },
  createUploadTask: native.createUploadTask,
}));

import { sendFromDisk } from './uploadFromDisk';

describe('sendFromDisk', () => {
  beforeEach(() => {
    native.cancel.mockClear();
    native.finish = undefined;
    native.fail = undefined;
    native.createUploadTask.mockReset().mockImplementation(() => ({
      uploadAsync: () => new Promise((resolve, reject) => { native.finish = resolve; native.fail = reject; }),
      cancelAsync: native.cancel,
    }));
  });

  it('sobe o arquivo do disco com os cabeçalhos do pedido e devolve a resposta do servidor', async () => {
    const headers = { 'Content-Type': 'application/zip', 'X-Filename': 'pacote.zip', Authorization: 'Bearer t' };
    const reply = sendFromDisk('file:///docs/pacote.zip')('http://maquina:8765/api/sessions/s/upload', { method: 'POST', headers });
    await vi.waitFor(() => expect(native.finish).toBeDefined());
    native.finish!({ status: 200, body: '{"path":"/up/pacote.zip"}', headers: { 'content-type': 'application/json' } });
    const res = await reply;
    expect(native.createUploadTask).toHaveBeenCalledExactlyOnceWith('http://maquina:8765/api/sessions/s/upload', 'file:///docs/pacote.zip',
      { httpMethod: 'POST', headers, uploadType: 0, sessionType: 1 });
    expect(res.status).toBe(200);
    expect(await res.json()).toEqual({ path: '/up/pacote.zip' });
  });

  it('erro HTTP do servidor chega como resposta, não como queda de rede', async () => {
    const reply = sendFromDisk('file:///docs/pacote.zip')('http://maquina:8765/api/sessions/s/upload', { method: 'POST', headers: {} });
    await vi.waitFor(() => expect(native.finish).toBeDefined());
    native.finish!({ status: 413, body: '{"detail":"grande demais"}', headers: {} });
    const res = await reply;
    expect(res.ok).toBe(false);
    expect(res.status).toBe(413);
  });

  it('prazo que estoura antes do envio começar não abre a tarefa nativa', async () => {
    const controller = new AbortController();
    const reply = sendFromDisk('file:///docs/pacote.zip')('http://maquina:8765/api/sessions/s/upload',
      { method: 'POST', headers: {}, signal: controller.signal });
    const reason = new DOMException('prazo', 'TimeoutError');
    controller.abort(reason);
    await expect(reply).rejects.toBe(reason);
    expect(native.createUploadTask).not.toHaveBeenCalled();
  });

  it('o prazo estourado cancela o envio nativo e rejeita com o motivo do sinal', async () => {
    const controller = new AbortController();
    const reply = sendFromDisk('file:///docs/pacote.zip')('http://maquina:8765/api/sessions/s/upload',
      { method: 'POST', headers: {}, signal: controller.signal });
    await vi.waitFor(() => expect(native.fail).toBeDefined());
    const reason = new DOMException('prazo', 'TimeoutError');
    controller.abort(reason);
    native.fail!(new Error('task was cancelled'));
    await expect(reply).rejects.toBe(reason);
    expect(native.cancel).toHaveBeenCalledOnce();
  });
});
