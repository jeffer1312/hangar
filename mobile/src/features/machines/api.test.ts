import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { configureApi } from '@hangar/core';
import { readIdentifier, readPeers, reason, registerBoth } from './api';
import * as m from '../../paraglide/messages';

const server = { id: 'api-owner', label: 'fixture-owner', baseUrl: 'https://owner.test', token: 'owner-secret' };
const fetchMock = vi.fn();
beforeEach(() => {
  fetchMock.mockReset(); vi.stubGlobal('fetch', fetchMock);
  configureApi({ getBaseUrl: () => 'https://wrong.test', getToken: () => 'wrong', origin: null, onUnauthorized: vi.fn(), createEventSource: vi.fn() });
});
afterEach(() => vi.unstubAllGlobals());

describe('respostas públicas de máquinas', () => {
  it.each([{ name: 'identificador', read: readIdentifier }, { name: 'peers', read: readPeers }])('traduz corpo HTML com HTTP 200 em resposta inválida: $name', async ({ read }) => {
    fetchMock.mockResolvedValue(new Response('<html>fixture-proxy</html>', { status: 200, headers: { 'Content-Type': 'text/html' } }));
    await expect(read(server, new AbortController().signal)).rejects.toThrow(m.mobile_machines_invalid_response());
  });

  it('preserva o cancelamento durante a leitura do corpo', async () => {
    const controller = new AbortController();
    const response = new Response('{}', { status: 200 });
    vi.spyOn(response, 'json').mockImplementation(async () => { controller.abort(); throw new DOMException('fixture-cancelled', 'AbortError'); });
    fetchMock.mockResolvedValue(response);
    await expect(readIdentifier(server, controller.signal)).rejects.toMatchObject({ name: 'AbortError' });
  });

  it('traduz a recusa pública de endereço duplicado do aparelho', () => {
    expect(reason(new Error('machine_address_duplicate'))).toBe(m.mobile_machines_duplicate_address());
  });
});

describe('identidade antes de registrar as duas pontas', () => {
  const remote = { id: 'api-remote', label: 'fixture-remote', baseUrl: 'https://remote.test', token: 'remote-secret' };
  const json = (value: unknown) => new Response(JSON.stringify(value), { status: 200, headers: { 'Content-Type': 'application/json' } });
  const respond = (nextIdentity: string) => {
    let remoteReads = 0;
    fetchMock.mockImplementation(async (url: string, init?: RequestInit) => {
      if (url.endsWith('/identificador')) {
        const identity = url.startsWith(remote.baseUrl) ? ++remoteReads === 1 ? 'remote' : nextIdentity : 'owner';
        return json({ identificador: identity, lan_url: '' });
      }
      if (url.includes('/check?')) return json({ estado: 'ok', identificador: new URL(url).searchParams.get('id'), motivo: '', tempo_ms: 4 });
      if (init?.method === 'POST') return json([]);
      return json({});
    });
  };

  it.each(['different', ''])('recusa identidade remota trocada ou vazia sem enviar o token do dono: %s', async (identity) => {
    respond(identity);
    const signal = new AbortController().signal;
    const found = await readIdentifier(remote, signal);
    let failure: unknown;
    try { await registerBoth(server, remote, found.identificador, signal); }
    catch (error) { failure = error; }
    expect(fetchMock.mock.calls.filter(([, init]) => init?.method === 'POST')).toHaveLength(0);
    expect(fetchMock.mock.calls.filter(([url]) => String(url).startsWith(remote.baseUrl)).every(([, init]) => !JSON.stringify(init).includes(server.token))).toBe(true);
    expect(reason(failure)).toBe(m.mobile_machines_other_machine({ expected: 'remote', actual: identity || '—' }));
  });

  it('confirma de novo a mesma identidade remota antes dos registros e mede os dois lados', async () => {
    respond('remote');
    const signal = new AbortController().signal;
    const found = await readIdentifier(remote, signal);
    const result = await registerBoth(server, remote, found.identificador, signal);
    const identityIndexes = fetchMock.mock.calls.flatMap(([url], index) => String(url) === `${remote.baseUrl}/api/peers/identificador` ? [index] : []);
    const firstPost = fetchMock.mock.calls.findIndex(([, init]) => init?.method === 'POST');
    expect(identityIndexes).toHaveLength(2);
    expect(identityIndexes[1]).toBeLessThan(firstPost);
    expect(fetchMock.mock.calls.filter(([, init]) => init?.method === 'POST')).toHaveLength(2);
    expect(result.partial).toBe('');
    expect(result.directions.forward?.estado).toBe('ok');
    expect(result.directions.back?.estado).toBe('ok');
  });

  const posted = () => fetchMock.mock.calls.filter(([, init]) => init?.method === 'POST')
    .map(([url, init]) => ({ url: String(url), token: JSON.parse(String(init?.body)).token as string }));
  const device = { ...server, token: 'device-token', credential: 'device' as const, deviceId: 'dev1' };

  it('nunca grava o token de aparelho em outra máquina: vai a senha guardada', async () => {
    respond('remote');
    const result = await registerBoth({ ...device, ownerToken: 'owner-secret' }, remote, 'remote', new AbortController().signal);
    expect(posted().find((call) => call.url.startsWith(remote.baseUrl))?.token).toBe('owner-secret');
    expect(result.partial).toBe('');
  });

  it('sem senha guardada pula essa direção e diz o motivo', async () => {
    respond('remote');
    const result = await registerBoth(device, remote, 'remote', new AbortController().signal);
    expect(posted().map((call) => call.url)).toEqual([`${server.baseUrl}/api/peers`]);
    expect(JSON.stringify(posted())).not.toContain('device-token');
    expect(result.partial).toBe(m.mobile_machines_peer_no_owner_password());
  });

  it('a outra máquina com token de aparelho também não vai parar aqui', async () => {
    respond('remote');
    const result = await registerBoth(server, { ...remote, token: 'remote-device', credential: 'device', deviceId: 'dev2' }, 'remote', new AbortController().signal);
    expect(posted().map((call) => call.url)).toEqual([`${remote.baseUrl}/api/peers`]);
    expect(result.partial).toBe(m.mobile_machines_peer_no_owner_password());
  });
});
