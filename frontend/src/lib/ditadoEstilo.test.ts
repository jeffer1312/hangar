// Corridas do estilo do ditado. GET e PATCH não têm ordem entre si, e nada impede dois cliques
// seguidos na folha — nos dois casos o defeito é o MESMO e é calado: a tela passa a mostrar um
// estilo que o servidor não tem, e nenhum caminho de código corrige depois.
import { beforeEach, describe, expect, it, vi } from 'vitest';

const getConfig = vi.fn();
const patchConfig = vi.fn();
vi.mock('@hangar/core', async (importOriginal) => ({
  ...(await importOriginal<typeof import('@hangar/core')>()),
  getConfig, patchConfig
}));

const { ditadoEstilo } = await import('./ditadoEstilo.svelte');

/** Promessa que só resolve quando MANDAREM — é o que permite escolher a ordem de chegada. */
function adiada<T>() {
  let resolver!: (v: T) => void;
  let rejeitar!: (e: unknown) => void;
  const promessa = new Promise<T>((res, rej) => { resolver = res; rejeitar = rej; });
  return { promessa, resolver, rejeitar };
}

const resposta = (v: string) => ({ campos: { ditado_estilo: { valor: v, definido: true, origem: 'app' } } });

beforeEach(() => {
  vi.clearAllMocks();
  // O store é singleton com cache: sem zerar, um teste herda o `carregado` do anterior.
  ditadoEstilo._zerarParaTeste();
});

describe('ditadoEstilo', () => {
  it('leitura atrasada NÃO pisa numa troca que aconteceu no meio', async () => {
    const get = adiada<ReturnType<typeof resposta>>();
    getConfig.mockReturnValue(get.promessa);
    patchConfig.mockResolvedValue({ campos: {} });

    const carga = ditadoEstilo.carregar();      // GET em voo, ainda sem resposta
    await ditadoEstilo.trocar('briefing');      // usuário troca e o PATCH conclui
    expect(ditadoEstilo.valor).toBe('briefing');

    get.resolver(resposta('limpar'));           // só agora chega o GET, com o valor VELHO
    await carga;

    expect(ditadoEstilo.valor).toBe('briefing');
  });

  it('troca que falha NÃO reverte por cima de uma troca posterior bem-sucedida', async () => {
    getConfig.mockResolvedValue(resposta('prosa'));
    await ditadoEstilo.carregar();

    const primeiro = adiada<unknown>();
    patchConfig.mockReturnValueOnce(primeiro.promessa);   // 1a troca: fica pendente
    patchConfig.mockResolvedValueOnce({ campos: {} });    // 2a troca: conclui na hora

    const t1 = ditadoEstilo.trocar('limpar').catch(() => {});
    await ditadoEstilo.trocar('briefing');
    expect(ditadoEstilo.valor).toBe('briefing');

    primeiro.rejeitar(new Error('502'));                  // a 1a falha DEPOIS
    await t1;

    expect(ditadoEstilo.valor).toBe('briefing');
  });

  it('troca que falha sozinha reverte pro valor anterior', async () => {
    getConfig.mockResolvedValue(resposta('prosa'));
    await ditadoEstilo.carregar();
    patchConfig.mockRejectedValue(new Error('502'));

    await expect(ditadoEstilo.trocar('briefing')).rejects.toThrow('502');
    expect(ditadoEstilo.valor).toBe('prosa');
  });

  it('falha de leitura não derruba nada: fica o padrão', async () => {
    getConfig.mockRejectedValue(new Error('rede'));
    await expect(ditadoEstilo.carregar()).resolves.toBeUndefined();
    expect(ditadoEstilo.valor).toBe('prosa');
  });

  it('getConfig estourando de forma SÍNCRONA não sobe pro $effect do Composer', async () => {
    // Erro síncrono escapa do encadeamento de promessas e viraria exceção não tratada dentro do
    // efeito que roda na montagem do Composer — ou seja, no caminho do microfone.
    getConfig.mockImplementation(() => { throw new TypeError('getConfig is not a function'); });
    await expect(ditadoEstilo.carregar()).resolves.toBeUndefined();
    expect(ditadoEstilo.valor).toBe('prosa');
  });

  it('valor estranho do servidor não vira estilo', async () => {
    getConfig.mockResolvedValue({ campos: { ditado_estilo: { valor: 42, definido: true, origem: 'app' } } });
    await ditadoEstilo.carregar();
    expect(ditadoEstilo.valor).toBe('prosa');
  });
});
