import { describe, it, expect, beforeEach, vi } from 'vitest';
import { configureApi } from './apiEnv';
import { fileUrl, fileUrlNative, fileAuthHeader, uploadUrl, uploadUrlNative } from './api';

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

  it('mutação: se fileUrlNative voltasse a anexar token, este teste ficaria vermelho', () => {
    const url = fileUrlNative('s', 'x.html');
    // esta asserção é a que falharia se alguém reintroduzisse &token= no nativo
    expect(url).not.toMatch(/token=/);
  });
});
