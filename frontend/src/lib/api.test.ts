import { beforeEach, describe, expect, it, vi } from 'vitest';
import { overwriteGetLocale } from '../paraglide/runtime';

const store = new Map<string, string>();
(globalThis as any).localStorage = {
  getItem: (k: string) => store.get(k) ?? null,
  setItem: (k: string, v: string) => store.set(k, String(v)),
  removeItem: (k: string) => store.delete(k),
};
(globalThis as any).document = { cookie: '' };
(globalThis as any).window = { location: { origin: 'https://app.test' } };

const { getConfig, getConfigForServer, patchConfig, patchConfigForServer, createSession, getHistory, isAbortError, transcribeFile } = await import('./api');
const { mensagemDeErro, formataErro } = await import('./errosApi');
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

// Round 4: o caminho GLOBAL (servidor ativo) também tinha o mesmo buraco do ForServer — servidor
// atrás de VPN não recusa conexão e o socket pendurava a folha de Configurações pra sempre.
describe('config global tem prazo (round 4)', () => {
  beforeEach(() => {
    store.set('cp_servers', JSON.stringify([server]));
    store.set('cp_active', server.id);
  });

  it('getConfig e patchConfig globais mandam signal por padrão', async () => {
    // Response nova por chamada: reusar o MESMO objeto faria o 2º res.json() estourar
    // ("Body has already been read").
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockImplementation(async () =>
      new Response(JSON.stringify({ campos: {}, somente_leitura: {} }), { status: 200 }),
    );
    await getConfig();
    expect(fetchMock.mock.calls[0][1]).toHaveProperty('signal');
    await patchConfig({ automations: false });
    expect(fetchMock.mock.calls[1][1]).toHaveProperty('signal');
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
  // O backend so aceita provider em ("claude", "codex", "pi", "kimi") e devolve 400 se vier `engine`
  // com provider != claude. O sheet manda engine/config_dir nulos fora do Claude — aqui garantimos que
  // o provider viaja LITERAL (a versao anterior tipava 'claude' | 'codex' e uma sessao Pi nem compilava).
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

  it('manda provider kimi literal no corpo (quarto provider)', async () => {
    store.set('cp_servers', JSON.stringify([server]));
    store.set('cp_active', server.id);
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockResolvedValue(
      new Response(JSON.stringify({ name: 'x', state: 'idle' }), { status: 200 }),
    );

    await createSession('x', '/home/eu/proj', null, 'kimi', null);

    const body = JSON.parse((fetchMock.mock.calls[0][1] as RequestInit).body as string);
    expect(body).toMatchObject({ name: 'x', cwd: '/home/eu/proj', provider: 'kimi', config_dir: null, engine: null });
  });
});

// Task 10 — errorDetail entende as DUAS formas do detail: string (endpoint nao migrado) e dict
// {code, params, msg} (backend/app/mensagens.py). O segundo caminho e o mais importante: backend
// novo com front velho (build em cache do service worker) cai no msg em portugues, nunca num codigo
// cru na tela.
describe('errorDetail (Task 10)', () => {
  beforeEach(() => {
    store.set('cp_servers', JSON.stringify([server]));
    store.set('cp_active', server.id);
    overwriteGetLocale(() => 'pt'); // mensagens m.* traduzidas no idioma fixado
  });

  it('detail em dict com code conhecido vira a mensagem traduzida', async () => {
    vi.spyOn(globalThis, 'fetch').mockResolvedValue(
      new Response(JSON.stringify({ detail: { code: 'erro_sem_paleta', params: {}, msg: 'sem paleta' } }), { status: 404 }),
    );
    // O caminho ForServer prefixa o status ('404: '); o sufixo ancorado garante que a mensagem
    // e a traduzida, nao o JSON cru (que terminaria em } e nao casaria).
    await expect(getConfigForServer(server)).rejects.toThrow(/sem paleta$/);
  });

  it('detail em dict com code DESCONHECIDO cai no msg em portugues', async () => {
    // Backend mais novo que o front: o code nao esta no mapa, mas o msg sempre chega junto.
    vi.spyOn(globalThis, 'fetch').mockResolvedValue(
      new Response(JSON.stringify({ detail: { code: 'code_futuro', params: { x: 1 }, msg: 'algo deu errado' } }), { status: 400 }),
    );
    await expect(getConfigForServer(server)).rejects.toThrow(/algo deu errado$/);
  });

  it('detail em string (endpoint nao migrado) continua funcionando como hoje', async () => {
    vi.spyOn(globalThis, 'fetch').mockResolvedValue(
      new Response(JSON.stringify({ detail: 'sessao nao existe' }), { status: 404 }),
    );
    await expect(getConfigForServer(server)).rejects.toThrow('sessao nao existe');
  });

  it('detail com code que e nome herdado do prototipo cai no msg, nao em [object Undefined]', async () => {
    // Parecer task 10, bloqueador 2: ERROS['toString'] devolve a funcao HERDADA do prototipo e a
    // chamada retorna '[object Undefined]' — so a leitura com hasOwnProperty faz codigo ausente
    // cair no msg em portugues, que e o contrato do mecanismo.
    vi.spyOn(globalThis, 'fetch').mockResolvedValue(
      new Response(JSON.stringify({ detail: { code: 'toString', params: {}, msg: 'mensagem fallback' } }), { status: 400 }),
    );
    await expect(getConfigForServer(server)).rejects.toThrow(/mensagem fallback$/);
  });
});

// Parecer task 10, bloqueador 3: sete valores pt entraram sem acento (o mapa os entrega direto na
// UI quando o code e conhecido). O teste fixa o locale pt e exige o texto com acento — e que
// codigos herdados do prototipo devolvem undefined em vez de quebrar ou chamar funcao errada.
describe('mensagemDeErro (parecer task 10)', () => {
  beforeEach(() => {
    overwriteGetLocale(() => 'pt');
  });

  it('erro_motor_invalido vem com acento em pt', () => {
    expect(mensagemDeErro('erro_motor_invalido')).toBe('motor inválido');
  });

  it('erro_tts_sem_cache vem com acento em pt', () => {
    expect(mensagemDeErro('erro_tts_sem_cache')).toBe('áudio não está mais em cache');
  });

  it('code herdado do prototipo devolve undefined, nao quebra nem chama funcao errada', () => {
    expect(mensagemDeErro('constructor')).toBeUndefined();
    expect(mensagemDeErro('__proto__')).toBeUndefined();
    expect(mensagemDeErro('hasOwnProperty')).toBeUndefined();
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

describe('formataErro (Task 11 round 2: envelopes nos helpers de envio e avisos do pareamento)', () => {
  it('string crua passa direto (endpoint antigo)', () => {
    expect(formataErro('sessao nao encontrada')).toBe('sessao nao encontrada');
  });

  it('envelope traduz pelo code no idioma do app', () => {
    overwriteGetLocale(() => 'en');
    const e = { code: 'erro_terminal_aberto', params: {}, msg: 'Terminal aberto nesta sessao' };
    expect(formataErro(e)).toBe('Terminal open in this session. Close the panel to reply here.');
  });

  it('envelope com code desconhecido cai no msg (rede)', () => {
    overwriteGetLocale(() => 'en');
    const e = { code: 'erro_desconhecido_futuro', params: {}, msg: 'texto legado cru' };
    expect(formataErro(e)).toBe('texto legado cru');
  });

  it('erro aninhado em params.erro traduz pelo contrato (fallback do /answer)', () => {
    overwriteGetLocale(() => 'en');
    const interno = { code: 'erro_fila_nao_entregue', params: {}, msg: 'fila indisponivel' };
    const t = mensagemDeErro('erro_drive_fallback_falhou', { erro: interno });
    expect(t).toBe('drive failed and text fallback too: queue unavailable and the prompt was not delivered');
  });

  it('lista de avisos {sessao, erro} formata cada erro traduzido (pareamento)', () => {
    overwriteGetLocale(() => 'pt');
    const avisos = [
      { sessao: 'me', erro: { code: 'erro_fila_nao_digitada', params: {}, msg: 'fila indisponivel' } },
      { sessao: 'voce', erro: 'falha de rede' },
    ];
    const t = mensagemDeErro('erro_pareamento_grupo_falha', { avisos });
    expect(t).toBe('falha em: me: fila indisponível e o prompt não foi digitado; voce: falha de rede');
  });

  it('um unico aviso nao deixa ; sobrando', () => {
    overwriteGetLocale(() => 'pt');
    const t = mensagemDeErro('erro_pareamento_aviso_parcial', {
      avisos: [{ sessao: 'voce', erro: 'falha de rede' }],
    });
    expect(t).toBe('aviso falhou em: voce: falha de rede');
  });

  it('array misto em ingles: envelope traduz, string de peer fica crua (parecer c0fc8a84)', () => {
    overwriteGetLocale(() => 'en');
    const t = mensagemDeErro('erro_pareamento_saida_falhou', {
      avisos: [
        { sessao: 'srv-a::x', erro: { code: 'erro_pareamento_server_id_ausente', params: {}, msg: 'CP_SERVER_ID ausente' } },
        { sessao: 'peer2', erro: 'rede caiu' },
      ],
    });
    expect(t).toBe(
      'leave notification failed: srv-a::x: CP_SERVER_ID missing in backend/.env — required for cross-server pairing (it\'s the reply address srv::sessao); peer2: rede caiu',
    );
  });

  it('não-envelope devolve undefined', () => {
    expect(formataErro(undefined)).toBeUndefined();
    expect(formataErro(42)).toBeUndefined();
  });
});
