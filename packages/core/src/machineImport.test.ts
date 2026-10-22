import { afterEach, beforeEach, describe, expect, test, vi } from 'vitest';
import { configureApi } from './apiEnv';
import {
  confirmImportedMachine, createMachineImportLink, discoverMachineCandidates, isLocalMachineUrl, machineImportIssue,
  mergeImportedMachines, normalizeMachineUrl, parseMachineImportPayload, readGroupIdentity, readMachineIdentity,
} from './machineImport';
import type { Server } from './servers';

const machine: Server = { id: 'home', label: 'fixture-home', baseUrl: 'https://home.test', token: 'secret%25/#?+' };
const credentialUrl = (baseUrl: string) => {
  const url = new URL(baseUrl);
  url.username = 'fixture-user';
  url.password = 'fixture-password';
  return url;
};

test('contexto seguro guarda somente host validado, sem corpo, caminho, token ou URL com credencial', () => {
  const error = Object.assign(new Error('leaked:secret-token'), { status: 403 });
  expect(machineImportIssue('peer_token', error, 'https://REMOTE.test:8765/private-path', 'https://source.test/private-path'))
    .toEqual({ stage: 'peer_token', status: 403, machine: 'remote.test:8765', source: 'source.test' });
  const url = credentialUrl('https://remote.test');
  url.searchParams.set('token', 'fixture-token');
  url.hash = 'fixture-fragment';
  expect(machineImportIssue('candidate', error, url.toString()))
    .toEqual({ stage: 'candidate', status: 403 });
});

describe('confirmação e redirecionamentos', () => {
  const fetchMock = vi.fn<typeof fetch>();
  const json = (body: unknown) => new Response(JSON.stringify(body), { headers: { 'Content-Type': 'application/json' } });
  beforeEach(() => {
    fetchMock.mockReset();
    vi.stubGlobal('fetch', fetchMock);
    configureApi({ getBaseUrl: () => 'https://active.test', getToken: () => 'active-token', origin: null,
      onUnauthorized: () => { throw new Error('unexpected_active_reset'); }, createEventSource: vi.fn() });
    fetchMock.mockImplementation(async (input) => String(input).endsWith('/api/sessions') ? json([]) : json({ identificador: 'confirmed-home', lan_url: '' }));
  });
  afterEach(() => vi.unstubAllGlobals());

  test('confirmação envia redirect error nas duas requisições da candidata', async () => {
    expect(await confirmImportedMachine(machine)).toEqual({ ...machine, label: 'confirmed-home', lan: { id: 'confirmed-home', url: '' } });
    expect(fetchMock.mock.calls.map(([url]) => url)).toEqual(['https://home.test/api/sessions', 'https://home.test/api/peers/identificador']);
    for (const [, init] of fetchMock.mock.calls) {
      expect(init?.redirect).toBe('error');
      expect(new Headers(init?.headers).get('Authorization')).toBe(`Bearer ${machine.token}`);
    }
  });

  test.each(['/api/sessions', '/api/peers/identificador'])('recusa da fronteira fetch por redirect rejeita confirmação em %s', async (path) => {
    fetchMock.mockImplementation(async (input, init) => {
      if (String(input).endsWith(path) && init?.redirect === 'error') throw new TypeError('fixture-redirect-refused');
      return String(input).endsWith('/api/sessions') ? json([]) : json({ identificador: 'confirmed-home', lan_url: '' });
    });
    await expect(confirmImportedMachine(machine)).rejects.toThrow('fixture-redirect-refused');
  });

  test('identidade avulsa e descoberta de peers, app-list e token conservam redirect padrão', async () => {
    fetchMock.mockImplementation(async (input) => {
      const url = String(input);
      if (url.endsWith('/api/peers')) return json([{ id: 'remote', base_url: 'https://remote.test', token: '••••mask' }]);
      if (url.endsWith('/app-list')) return json({ rev: 'r1', servers: [] });
      if (url.endsWith('/token')) return json({ token: 'fixture-remote-token' });
      return json({ identificador: 'confirmed-home', lan_url: '' });
    });
    await readMachineIdentity(machine);
    const result = await discoverMachineCandidates([machine], () => [machine]);
    expect(result.candidates).toEqual([{ id: 'remote', label: 'remote', baseUrl: 'https://remote.test', token: 'fixture-remote-token' }]);
    expect(fetchMock.mock.calls.map(([url]) => url)).toEqual([
      'https://home.test/api/peers/identificador', 'https://home.test/api/peers',
      'https://home.test/api/peers/app-list', 'https://home.test/api/peers/remote/token',
    ]);
    for (const [, init] of fetchMock.mock.calls) expect(init).not.toHaveProperty('redirect');
  });

  test('identificador de agrupamento: lido da rota; vazio é falha `vazio`, como erro de rede', async () => {
    expect(await readGroupIdentity(machine)).toBe('confirmed-home');
    fetchMock.mockImplementation(async () => json({ identificador: '', lan_url: '' }));
    await expect(readGroupIdentity(machine)).rejects.toMatchObject({ name: 'vazio' });
    fetchMock.mockImplementation(async () => new Response('', { status: 500 }));
    await expect(readGroupIdentity(machine, new AbortController().signal)).rejects.toMatchObject({ status: 500 });
  });
});

describe('payload de importação de máquinas', () => {
  test('lê JSON já decodificado, normaliza endereço e não decodifica o token outra vez', () => {
    expect(parseMachineImportPayload(JSON.stringify([{ ...machine, baseUrl: 'HTTPS://HOME.TEST:443///' }])))
      .toEqual([machine]);
  });

  test.each(['', '{', 'null', '{}', '[null]', JSON.stringify([{ ...machine, token: '' }]),
    JSON.stringify([{ ...machine, id: 3 }]), JSON.stringify([{ ...machine, label: null }]),
    JSON.stringify([{ ...machine, token: 'a b' }]), JSON.stringify([{ ...machine, token: 'x'.repeat(8193) }]),
    JSON.stringify(Array.from({ length: 101 }, () => machine)), ' '.repeat(262145),
  ])('recusa payload inválido sem reproduzir credenciais no erro', (raw) => {
    expect(() => parseMachineImportPayload(raw)).toThrow('machine_import_invalid_payload');
    try { parseMachineImportPayload(raw); } catch (error) { expect(String(error)).not.toContain(machine.token); }
  });

  test.each(['ftp://home.test', credentialUrl('https://home.test').toString(), 'https://home.test?token=fixture-token',
    'https://home.test#fixture-fragment', '//home.test', 'https://home.test?', 'https://home.test#',
  ])('recusa URL fora do contrato: %s', (baseUrl) => {
    expect(() => parseMachineImportPayload(JSON.stringify([{ ...machine, baseUrl }]))).toThrow();
  });

  test('deixa convites, temporárias, removidas e loopback fora, sem invalidar as próprias', () => {
    expect(parseMachineImportPayload(JSON.stringify([
      { ...machine, invite: true }, { ...machine, ephemeral: true }, { ...machine, removed: true },
      { ...machine, baseUrl: 'http://127.0.0.1:8765' }, machine, { ...machine, token: 'other' },
    ]))).toEqual([machine]);
  });

  test('link usa só fragmento e apenas os quatro campos públicos da transferência', () => {
    const link = createMachineImportLink([
      { ...machine, disabled: true, lan: { url: 'http://10.0.0.3', id: 'home' } },
      { ...machine, id: 'guest', invite: true }, { ...machine, ephemeral: true }, { ...machine, removed: true },
    ]);
    expect(link.startsWith('hangar://importar#')).toBe(true);
    expect(new URL(link).search).toBe('');
    expect(JSON.parse(decodeURIComponent(new URL(link).hash.slice(1)))).toEqual([machine]);
  });
});

describe('endereços de importação', () => {
  test.each(['http://localhost:8765', 'http://a.localhost.', 'http://127.3.2.1', 'http://127.1',
    'http://2130706433', 'http://0x7f000001', 'http://0.0.0.0', 'http://0', 'http://[::1]',
    'http://[0:0:0:0:0:0:0:1]', 'http://[::]', 'http://[::ffff:127.0.0.1]',
    'http://[::ffff:7f01:203]', 'http://[::ffff:0:0]', 'http://*', 'http://+', 'http://[::127.0.0.1]',
  ])('reconhece endereço desta máquina: %s', (url) => expect(isLocalMachineUrl(normalizeMachineUrl(url))).toBe(true));
  test.each(['http://192.168.0.3:8765', 'https://home.ts.net', 'http://[fd00::1]:8765'])
    ('permite máquina alcançável: %s', (url) => expect(isLocalMachineUrl(normalizeMachineUrl(url))).toBe(false));
});

describe('merge só acrescenta', () => {
  test.each(['https://home.tail.ts.net', 'http://100.64.0.2:8765'])('guarda o Tailscale da mesma máquina como alternativa sem duplicar o cadastro local: %s', (baseUrl) => {
    const saved = { ...machine, baseUrl: 'http://192.168.1.2:8765', lan: { id: 'confirmed-home', url: 'http://192.168.1.2:8765' } };
    const remote = { ...machine, id: 'alias', baseUrl, lan: { id: 'confirmed-home', url: saved.baseUrl } };

    const result = mergeImportedMachines({ servers: [saved], activeId: saved.id }, [remote]);

    expect(result.servers).toHaveLength(1);
    expect(result.servers[0].baseUrl).toBe(saved.baseUrl);
    expect(result.servers[0].remote).toEqual({ url: remote.baseUrl, id: 'confirmed-home' });
    expect(result.added).toEqual([]);
    expect(result.activeId).toBe(saved.id);
  });

  test('mesma identidade confirmada em outro endereço não duplica nem usa o apelido como identidade', () => {
    const saved = { ...machine, label: 'fixture-custom-nickname', disabled: true, lan: { id: 'confirmed-home', url: '' } };
    const alias = { ...machine, id: 'alias', baseUrl: 'https://home-alias.test', label: 'confirmed-home', token: 'alias-token', lan: { id: 'confirmed-home', url: 'http://192.168.0.2' } };
    const result = mergeImportedMachines({ servers: [saved], activeId: saved.id }, [alias]);
    expect(result.added).toEqual([]);
    expect(result.servers).toEqual([saved]);
    expect(result.servers[0]).toBe(saved);
    expect(result.activeId).toBe(saved.id);
  });

  test('URLs distintas da mesma identidade na rodada só acrescentam a primeira e não removem duplicatas existentes', () => {
    const first = { ...machine, lan: { id: 'confirmed-home', url: '' } };
    const alias = { ...first, id: 'alias', baseUrl: 'https://home-alias.test' };
    const result = mergeImportedMachines({ servers: [], activeId: null }, [first, alias]);
    expect(result.servers).toEqual([first]);
    expect(result.activeId).toBe(first.id);
    const before = [first, alias];
    const repeated = mergeImportedMachines({ servers: before, activeId: alias.id }, [alias]);
    expect(repeated.servers).toBe(before);
    expect(repeated.activeId).toBe(alias.id);
  });

  test('identidades vazias, apelidos iguais e identidade de convidado não escondem máquinas próprias diferentes', () => {
    const first = { ...machine, lan: { id: '', url: '' } };
    const second = { ...first, id: 'second', baseUrl: 'https://second.test' };
    expect(mergeImportedMachines({ servers: [], activeId: null }, [first, second]).added).toHaveLength(2);
    const guest = { ...first, id: 'guest', invite: true, lan: { id: 'shared-id', url: '' } };
    const own = { ...second, lan: { id: 'shared-id', url: '' } };
    expect(mergeImportedMachines({ servers: [guest], activeId: guest.id }, [own]).added).toEqual([own]);
  });

  test('metadado lan alegado pelo payload não vira identidade confirmada', () => {
    expect(parseMachineImportPayload(JSON.stringify([{ ...machine, lan: { id: 'claimed-id', url: 'https://claimed.test' } }]))).toEqual([machine]);
  });
  test('preserva lista, token, apelido, flags e ativa; não colide IDs locais', () => {
    const saved = { ...machine, disabled: true, lan: { url: '', id: 'home' } };
    const next = { ...machine, id: 'home', baseUrl: 'https://work.test', label: 'fixture-work', token: 'work' };
    const result = mergeImportedMachines({ servers: [saved], activeId: saved.id }, [
      { ...machine, label: 'overwrite', token: 'overwrite', baseUrl: 'https://HOME.test/' }, next, next,
    ]);
    expect(result.servers[0]).toBe(saved);
    expect(result.servers).toHaveLength(2);
    expect(result.servers[1]).toMatchObject({ baseUrl: next.baseUrl, token: 'work' });
    expect(result.servers[1].id).not.toBe('home');
    expect(result.activeId).toBe('home');
    expect(result.added).toEqual([result.servers[1]]);
  });

  test('primeira nova vira ativa sem escolher uma entrada antiga nem substituir convidado no mesmo endereço', () => {
    const guest = { ...machine, id: 'guest', invite: true };
    const result = mergeImportedMachines({ servers: [guest], activeId: null }, [machine,
      { ...machine, id: 'work', baseUrl: 'https://work.test' },
    ]);
    expect(result.servers).toEqual([guest, { ...machine, id: 'work', baseUrl: 'https://work.test' }]);
    expect(result.activeId).toBe('work');
  });

  test('não altera a lista nem o ativo quando tudo já está salvo', () => {
    const list = [machine];
    const result = mergeImportedMachines({ servers: list, activeId: machine.id }, [machine]);
    expect(result.servers).toBe(list);
    expect(result.added).toEqual([]);
    expect(result.activeId).toBe('home');
  });
});
