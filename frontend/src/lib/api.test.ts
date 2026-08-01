import { beforeEach, describe, expect, it, vi } from 'vitest';

const store = new Map<string, string>();
(globalThis as any).localStorage = {
  getItem: (k: string) => store.get(k) ?? null,
  setItem: (k: string, v: string) => store.set(k, String(v)),
  removeItem: (k: string) => store.delete(k),
};
(globalThis as any).document = { cookie: '' };
(globalThis as any).window = { location: { origin: 'https://app.test' } };

const { getConfigForServer, patchConfigForServer, createSession, getHistory, isAbortError, transcribeFile } = await import('./api');
const { listServers, getActiveId } = await import('./auth');
const server = { id: 'a', label: 'Servidor A', baseUrl: 'https://a.test', token: 'token-a' };

beforeEach(() => {
  vi.restoreAllMocks();
});

describe('explicit server settings API', () => {
  it('usa base e token explícitos sem depender do servidor ativo', async () => {
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockResolvedValue(
      new Response(JSON.stringify({ campos: {}, somente_leitura: {} }), { status: 200 }),
    );

    await getConfigForServer(server);

    expect(fetchMock).toHaveBeenCalledWith('https://a.test/api/config', expect.objectContaining({
      headers: expect.objectContaining({ Authorization: 'Bearer token-a' }),
    }));
  });

  it('401 explícito vira erro e não recarrega nem remove credencial global', async () => {
    // O cenário que importa: o servidor ATIVO é OUTRA máquina. Sem montá-lo, o teste passaria
    // mesmo que apiFetchForServer chamasse dropActiveServer — não haveria credencial pra derrubar.
    const outra = { id: 'b', label: 'Servidor B', baseUrl: 'https://b.test', token: 'token-b' };
    store.set('cp_servers', JSON.stringify([outra]));
    store.set('cp_active', outra.id);

    const reload = vi.fn();
    (globalThis as any).window.location.reload = reload;
    vi.spyOn(globalThis, 'fetch').mockResolvedValue(
      new Response(JSON.stringify({ detail: 'token inválido' }), { status: 401 }),
    );

    await expect(patchConfigForServer(server, { automations: false }))
      .rejects.toThrow('401: token inválido');

    expect(reload).not.toHaveBeenCalled();
    expect(listServers()).toEqual([outra]);   // a credencial da outra máquina segue intacta
    expect(getActiveId()).toBe(outra.id);
  });

  // 502 de infra (proxy Tailscale) sem corpo JSON e sem statusText (comum atras de HTTP/2): sem o
  // fallback, `.message` fica '' e telas que testam `if (erro)` (TtsBar, ServerSettings) desenham a
  // UI de sucesso por cima de uma falha real.
  it('erro sem corpo e sem statusText ainda produz mensagem não-vazia', async () => {
    vi.spyOn(globalThis, 'fetch').mockResolvedValue(
      new Response('', { status: 502, statusText: '' }),
    );

    await expect(getConfigForServer(server)).rejects.toThrow(/502/);
  });
});

// O sintoma real: com um servidor da malha OFFLINE, abrir Configuracoes dele prendia a folha em
// "Carregando..." pra sempre. VPN pra no morto nao RECUSA a conexao — o socket fica pendurado e a
// promessa nunca resolve, entao nem o catch nem o finally da tela rodavam.
describe('chamada a outro servidor tem prazo', () => {
  it('manda um signal por padrão', async () => {
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockResolvedValue(
      new Response(JSON.stringify({ campos: {}, somente_leitura: {} }), { status: 200 }),
    );
    await getConfigForServer(server);
    expect(fetchMock.mock.calls[0][1]).toHaveProperty('signal');
  });

  it('estourar o prazo vira erro legível, com o nome do servidor', async () => {
    vi.spyOn(globalThis, 'fetch').mockRejectedValue(
      new DOMException('signal timed out', 'TimeoutError'),
    );
    await expect(getConfigForServer(server)).rejects.toThrow(/Servidor A.*não respondeu/);
  });

  it('quem CANCELA por conta própria recebe o abort cru, não a mensagem de servidor fora do ar', async () => {
    vi.spyOn(globalThis, 'fetch').mockRejectedValue(new DOMException('aborted', 'AbortError'));
    await expect(getConfigForServer(server)).rejects.toSatisfy(
      (e: unknown) => e instanceof DOMException && (e as DOMException).name === 'AbortError',
    );
  });
});

describe('getHistory', () => {
  // fetch que respeita o signal, como o do browser: só termina quando a resposta chega OU quando o
  // signal aborta — é o que permite provar que o download PARA, e não só que a resposta é ignorada.
  function fetchQueRespeitaOSignal() {
    return vi.spyOn(globalThis, 'fetch').mockImplementation((_url, init) =>
      new Promise((_resolve, reject) => {
        const s = (init as RequestInit).signal!;
        s.addEventListener('abort', () => reject(s.reason));
      }),
    );
  }

  beforeEach(() => {
    store.set('cp_servers', JSON.stringify([server]));
    store.set('cp_active', server.id);
  });

  it('abortar cancela o fetch de verdade e NÃO vira erro de tela', async () => {
    const fetchMock = fetchQueRespeitaOSignal();
    const ctl = new AbortController();

    const p = getHistory('sessao', undefined, ctl.signal);
    ctl.abort();
    const err = await p.catch((e) => e);

    // 1. o fetch recebeu um signal que de fato abortou -> a requisição para na rede
    const passado = (fetchMock.mock.calls[0][1] as RequestInit).signal!;
    expect(passado.aborted).toBe(true);
    // 2. o rejeito é cancelamento, não falha -> o Chat retorna sem pintar pílula de erro
    expect(isAbortError(err)).toBe(true);
    // 3. falha de verdade continua sendo falha (inclusive o timeout de 45s, que é TimeoutError)
    expect(isAbortError(new Error('500: boom'))).toBe(false);
    expect(isAbortError(new DOMException('demorou', 'TimeoutError'))).toBe(false);
  });

  it('limit=0 pede zero eventos, não o arquivo inteiro', async () => {
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockResolvedValue(
      new Response('[]', { status: 200 }),
    );

    await getHistory('sessao', 0);

    expect(fetchMock.mock.calls[0][0]).toBe('https://a.test/api/sessions/sessao/history?limit=0');
  });
});

describe('createSession', () => {
  // O backend so aceita provider em ("claude", "codex", "pi") e devolve 400 se vier `engine` com
  // provider != claude. O sheet manda engine/config_dir nulos fora do Claude — aqui garantimos que o
  // provider viaja LITERAL (a versao anterior tipava 'claude' | 'codex' e uma sessao Pi nem compilava).
  it('manda o provider escolhido no corpo, sem motor', async () => {
    store.set('cp_servers', JSON.stringify([server]));
    store.set('cp_active', server.id);
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockResolvedValue(
      new Response(JSON.stringify({ name: 'x', state: 'idle' }), { status: 200 }),
    );

    await createSession('x', '/home/eu/proj', null, 'pi', null);

    const body = JSON.parse((fetchMock.mock.calls[0][1] as RequestInit).body as string);
    expect(fetchMock.mock.calls[0][0]).toBe('https://a.test/api/sessions');
    expect(body).toMatchObject({ name: 'x', cwd: '/home/eu/proj', provider: 'pi', config_dir: null, engine: null });
  });
});

// A corrente toggleRecord -> addFiles({ditado:true}) -> transcribeIntoComposer -> transcribeFile
// so pode acender `limpar=1` no pedido do mic. Este e o elo mais barato de quebrar (um `{ditado:
// true}` esquecido no caminho do anexo manda audio de 10min pro LLM) e o unico sem teste algum.
describe('transcribeFile', () => {
  beforeEach(() => {
    store.set('cp_servers', JSON.stringify([server]));
    store.set('cp_active', server.id);
  });

  it('so manda ?limpar=1 quando pedido explicitamente', async () => {
    // Response.json() so le o corpo uma vez — mockResolvedValue reusaria a MESMA Response nas 3
    // chamadas, então uma nova resposta a cada invocação.
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockImplementation(async () =>
      new Response(JSON.stringify({ path: 'p', text: 't' }), { status: 200 }),
    );
    const file = new File(['x'], 'audio.webm');

    await transcribeFile('sessao', file, { limpar: true });
    expect(fetchMock.mock.calls[0][0]).toBe('https://a.test/api/sessions/sessao/transcribe?limpar=1');

    await transcribeFile('sessao', file);
    expect(fetchMock.mock.calls[1][0]).toBe('https://a.test/api/sessions/sessao/transcribe');

    await transcribeFile('sessao', file, {});
    expect(fetchMock.mock.calls[2][0]).toBe('https://a.test/api/sessions/sessao/transcribe');
  });
});
