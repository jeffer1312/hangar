import { describe, it, expect, beforeEach, vi } from 'vitest';

// Mesmo stub do background.test.ts: o modulo escreve no <html>, e env=node nao tem DOM.
const varsCss = new Map<string, string>();
const docListeners: Record<string, ((e?: any) => void)[]> = {};
(globalThis as any).document = {
  documentElement: {
    style: {
      setProperty: (k: string, v: string) => varsCss.set(k, v),
      removeProperty: (k: string) => varsCss.delete(k),
    },
    dataset: {},
  },
  querySelector: () => null,
  hidden: false,
  addEventListener: (ev: string, cb: any) => {
    (docListeners[ev] ??= []).push(cb);
  },
};
(globalThis as any).localStorage = { getItem: () => null, setItem: () => {}, removeItem: () => {} };

// `ligarAtualizacaoAoFocar` precisa de window (foco) + document.visibilitychange, e `ehLocal` de
// `location`. Nao ha jsdom aqui, entao os tres sao stubs minimos.
const winListeners: Record<string, ((e?: any) => void)[]> = {};
(globalThis as any).window = {
  addEventListener: (ev: string, cb: any) => {
    (winListeners[ev] ??= []).push(cb);
  },
};
(globalThis as any).location = { origin: 'http://127.0.0.1:8765' };

// fetch controlavel por teste — troca o corpo entre os `it`s, sem precisar de rede real.
let fetchImpl: (...args: any[]) => Promise<any> = async () => {
  throw new Error('fetchImpl nao configurado neste teste');
};
(globalThis as any).fetch = (...args: any[]) => fetchImpl(...args);

// `desktopTheme.ts` so consome getBaseUrl/getToken — mockar `./auth` inteiro evita montar um
// localStorage stateful so pra simular servidores cadastrados.
const mockGetBaseUrl = vi.fn(() => '');
const mockGetToken = vi.fn(() => 'tok');
vi.mock('./auth', () => ({
  getBaseUrl: () => mockGetBaseUrl(),
  getToken: () => mockGetToken(),
}));

const { mapear, aplicarPaleta, limparPaleta, CHAVES, ehLocal, buscarPaleta, ligarAtualizacaoAoFocar, paletaEmCache } =
  await import('./desktopTheme');

const AZUL = {
  escuro: true,
  cores: {
    background: '#111318', surface: '#111318', surfaceContainerLow: '#191C20',
    surfaceContainer: '#1D2024', surfaceContainerHigh: '#282A2F',
    onSurface: '#E2E2E9', onSurfaceVariant: '#C4C6D0',
    outline: '#8E9099', outlineVariant: '#44474E',
    primary: '#AAC7FF', onPrimary: '#0A305F',
  },
};

describe('tema do desktop', () => {
  beforeEach(() => varsCss.clear());

  it('mapeia neutros, destaque e as triplas de vidro', () => {
    const m = mapear(AZUL, false);
    expect(m['--bg-base']).toBe('#111318');
    expect(m['--bg-surface']).toBe('#191C20');
    expect(m['--bg-elevated']).toBe('#1D2024');
    expect(m['--bg-hover']).toBe('#282A2F');
    expect(m['--accent']).toBe('#AAC7FF');
    // Sem as triplas, painel/navbar/composer ficariam no indigo enquanto o resto vira Material.
    expect(m['--glass-panel-rgb']).toBe('25 28 32');   // #191C20
    expect(m['--glass-rgb']).toBe('40 42 47');         // #282A2F
    expect(m['--glass-solid-rgb']).toBe('17 19 24');   // #111318
    // Veu do body::after: MESMA fonte do --bg-base (c.background) — sem isto o veu ficava indigo
    // fixo por baixo de caixas ja na cor do papel de parede.
    expect(m['--veu-rgb']).toBe('17 19 24');           // #111318
    // Veu da amostra: MESMA fonte, nome PROPRIO — sem isto, ligar o tema Desktop deixava a previa
    // de Aparencia mostrando uma cor que a tela real ja nao tinha mais.
    expect(m['--veu-amostra-rgb']).toBe('17 19 24');   // #111318
  });

  it('o destaque pressionado e mais escuro que o normal', () => {
    // Igual ao --accent, o botao nao daria retorno nenhum ao ser apertado.
    const m = mapear(AZUL, false);
    expect(m['--accent-press']).not.toBe(m['--accent']);
  });

  it('as bordas mantem a leveza de hoje', () => {
    // As de hoje sao rgba(255,248,244,0.07/0.12). Cor opaca aqui deixaria a tela riscada.
    const m = mapear(AZUL, false);
    expect(m['--border-subtle']).toMatch(/^rgba\(.*0\.0[0-9]\)$/);
    expect(m['--border-default']).toMatch(/^rgba\(/);
  });

  it('sem "texto do desktop", as letras do app ficam de fora', () => {
    const m = mapear(AZUL, false);
    expect(m['--text-primary']).toBeUndefined();
    expect(m['--text-secondary']).toBeUndefined();
    expect(m['--text-muted']).toBeUndefined();
  });

  it('com "texto do desktop", as letras e as copias -base entram', () => {
    const m = mapear(AZUL, true);
    expect(m['--text-primary']).toBe('#E2E2E9');
    expect(m['--text-secondary']).toBe('#C4C6D0');
    expect(m['--text-muted']).toBe('#8E9099');
    // Sem as copias -base, ligar papel de parede misturaria a partir da cor VELHA (app.css:299).
    expect(m['--text-primary-base']).toBe('#E2E2E9');
  });

  it('semantica, estado e grafico NUNCA entram', () => {
    const m = mapear(AZUL, true);
    for (const k of ['--success', '--error', '--warning', '--chart-2', '--chart-3', '--chart-4',
                     '--pair-l', '--pill-working-bg', '--pill-working-fg']) {
      expect(m[k]).toBeUndefined();
    }
    // Contagem: sem isto, um `mapear` que devolvesse {} passaria em todos os `toBeUndefined`.
    expect(Object.keys(m).length).toBe(CHAVES.length);
  });

  it('aplicar escreve no html e limpar tira TODAS as chaves', () => {
    aplicarPaleta(AZUL, true);
    expect(varsCss.get('--bg-base')).toBe('#111318');
    expect(document.documentElement.dataset.theme).toBe('dark');
    limparPaleta();
    for (const k of CHAVES) expect(varsCss.get(k)).toBeUndefined();
  });

  it('darkmode false leva o data-theme pro claro', () => {
    aplicarPaleta({ ...AZUL, escuro: false }, false);
    expect(document.documentElement.dataset.theme).toBe('light');
  });

  // Fix 4 da revisao: `mapear` roda ANTES de tocar no DOM. Uma paleta malformada que jogue dentro
  // dele (ex: `cores` ausente, TypeError ao ler `c.background`) nao pode deixar o tema flipado com
  // as cores velhas ainda escritas — nada parcial observavel.
  it('paleta malformada: mapear() joga e dataset.theme fica intocado', () => {
    document.documentElement.dataset.theme = 'light';
    const malformada = { escuro: true } as unknown as Parameters<typeof aplicarPaleta>[0];
    expect(() => aplicarPaleta(malformada, false)).toThrow();
    expect(document.documentElement.dataset.theme).toBe('light');
    expect(varsCss.get('--bg-base')).toBeUndefined();
  });

  it('preto puro clareia em vez de ficar igual (--accent-press nunca == --accent)', () => {
    const m = mapear({ ...AZUL, cores: { ...AZUL.cores, primary: '#000000' } }, false);
    expect(m['--accent-press']).toMatch(/^#[0-9a-f]{6}$/i);
    expect(m['--accent-press']).not.toBe('#000000');
  });

  it('branco puro escurece normalmente', () => {
    const m = mapear({ ...AZUL, cores: { ...AZUL.cores, primary: '#FFFFFF' } }, false);
    expect(m['--accent-press']).toMatch(/^#[0-9a-f]{6}$/i);
    expect(m['--accent-press']).not.toBe('#FFFFFF');
  });
});

// O gate real: a pagina pode ter vindo do localhost com um servidor REMOTO ativo — o caso que
// `ehLocal` existe pra barrar, senao o app pediria a paleta (e o wallpaper) da maquina errada.
describe('ehLocal', () => {
  it('true quando o servidor ativo nao tem baseUrl (mesma origem por convencao)', () => {
    mockGetBaseUrl.mockReturnValue('');
    expect(ehLocal()).toBe(true);
  });

  it('true quando o baseUrl bate com a origem da pagina', () => {
    mockGetBaseUrl.mockReturnValue('http://127.0.0.1:8765');
    expect(ehLocal()).toBe(true);
  });

  it('false quando o baseUrl e de outra maquina — pagina local, servidor ativo remoto', () => {
    mockGetBaseUrl.mockReturnValue('http://outra-maquina.ts.net:8765');
    expect(ehLocal()).toBe(false);
  });
});

describe('buscarPaleta', () => {
  beforeEach(() => {
    mockGetBaseUrl.mockReturnValue(''); // ehLocal() = true nos quatro testes
    mockGetToken.mockReturnValue('tok');
  });

  it('404 (maquina sem rice) -> null', async () => {
    fetchImpl = async () => ({ ok: false, status: 404 });
    expect(await buscarPaleta()).toBeNull();
  });

  it('403 (nao e a maquina) -> null', async () => {
    fetchImpl = async () => ({ ok: false, status: 403 });
    expect(await buscarPaleta()).toBeNull();
  });

  it('fetch rejeitando (backend fora do ar) -> null, nao trava a abertura do app', async () => {
    fetchImpl = async () => {
      throw new Error('ECONNREFUSED');
    };
    expect(await buscarPaleta()).toBeNull();
  });

  it('resposta ok -> devolve a paleta', async () => {
    fetchImpl = async () => ({ ok: true, json: async () => AZUL });
    expect(await buscarPaleta()).toEqual(AZUL);
  });

  // Fix 4 da revisao: hoje SO este backend responde a rota, mas o app fala com varios servidores —
  // 200 com uma chave faltando (versao velha do backend, ou qualquer coisa respondendo na mesma
  // rota) tem que virar null, a MESMA resposta de um 404, nao um objeto que `mapear` quebra em
  // cima depois.
  it('200 com uma chave que mapear() le faltando -> null', async () => {
    const semPrimary = { ...AZUL, cores: { ...AZUL.cores } };
    delete (semPrimary.cores as Record<string, string>).primary;
    fetchImpl = async () => ({ ok: true, json: async () => semPrimary });
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    expect(await buscarPaleta()).toBeNull();
    expect(warn).toHaveBeenCalled();   // rastro de dev — sem isto o desenvolvedor nao teria pista
    warn.mockRestore();
  });

  it('200 sem "cores" nenhum -> null, nao levanta', async () => {
    fetchImpl = async () => ({ ok: true, json: async () => ({ escuro: true }) });
    vi.spyOn(console, 'warn').mockImplementation(() => {});
    expect(await buscarPaleta()).toBeNull();
  });

  // Fix 3 da revisao: cache de modulo. Uma falha DEPOIS de um sucesso nao pode apagar o que ja se
  // sabia, senao ThemeToggle/AppearanceSettings ficam mudos assim que o backend pisca uma vez.
  it('cacheia a ultima paleta com sucesso, e uma falha depois nao apaga o cache', async () => {
    fetchImpl = async () => ({ ok: true, json: async () => AZUL });
    await buscarPaleta();
    expect(paletaEmCache()).toEqual(AZUL);

    fetchImpl = async () => { throw new Error('ECONNREFUSED'); };
    expect(await buscarPaleta()).toBeNull();
    expect(paletaEmCache()).toEqual(AZUL);
  });
});

// `ligado` e flag de MODULO (uma vez ligado, fica ligado pro processo inteiro) — entao registro e
// comportamento tem que ser verificados no MESMO teste, com um predicate MUTAVEL: uma segunda
// chamada com um predicate diferente nao substitui o primeiro, ela e descartada pelo guard.
describe('ligarAtualizacaoAoFocar', () => {
  it('registra os listeners uma unica vez, e so rebusca quando o predicate diz "ativo"', async () => {
    mockGetBaseUrl.mockReturnValue('');
    mockGetToken.mockReturnValue('tok');
    let chamadas = 0;
    fetchImpl = async () => {
      chamadas++;
      return { ok: true, json: async () => AZUL };
    };
    let ativo = false;

    ligarAtualizacaoAoFocar(() => ativo, () => true);
    // Segunda chamada (ex: HMR, remount) tem que ser no-op — sem isto o app dispararia N fetches
    // por foco, o bug exato que este teste existe pra travar.
    ligarAtualizacaoAoFocar(() => ativo, () => true);
    expect(winListeners['focus']?.length).toBe(1);
    expect(docListeners['visibilitychange']?.length).toBe(1);

    const focar = winListeners['focus'][0];

    // Tema do desktop inativo (app no claro/padrao) -> focar a janela nao pode bater no endpoint.
    focar();
    await new Promise((r) => setTimeout(r, 0));
    expect(chamadas).toBe(0);

    // Tema ativo -> agora sim, focar rebusca e reaplica.
    ativo = true;
    focar();
    await new Promise((r) => setTimeout(r, 0));
    expect(chamadas).toBe(1);
    expect(varsCss.get('--accent')).toBe(AZUL.cores.primary);
  });
});
