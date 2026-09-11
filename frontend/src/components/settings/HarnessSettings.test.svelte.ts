// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { mount, tick, unmount } from 'svelte';
import HarnessSettings from './HarnessSettings.svelte';
import type { Server } from '../../lib/auth';
import type { IntegracaoCodex } from '../../lib/credenciais';
import * as m from '../../paraglide/messages';

const A: Server = { id: 'a', label: 'A', baseUrl: 'http://a.local', token: 'token-a' };
const B: Server = { id: 'b', label: 'B', baseUrl: 'http://b.local', token: 'token-b' };
const ROTA = '/api/harness/codex/integracao';
const ROTA_INST = '/api/harness/instalar';
const ROTA_CONFIG = '/api/config';
const ROTA_CODEX = '/api/harness/codex/opcoes';
// A tela também consulta a instalação na montagem (é de lá que sai a lista de quem dá pra instalar
// por botão). Sem uma resposta com a forma certa, o `fetch` genérico abaixo devolveria a lista de
// harnesses no lugar dela.
const ociosa = {
  fase: 'ocioso', harness: null, etapa: null, passo: 0, total: 4, log: [], ok: null, erro: null,
  comandos: {}, manual: {},
};
const estado = (dados: Partial<IntegracaoCodex> = {}): IntegracaoCodex => ({
  estado: 'ocioso', etapa: '', ultima_execucao: null, proxima_atualizacao: null,
  plugins: [], erros: [], avisos: [], confianca_pendente: false, automatica: true, ...dados,
});
const resposta = (dados: unknown) => ({ ok: true, status: 200, json: async () => dados }) as Response;
const harnesses = ['codex', 'claude'].map((id) => ({ id, nome: id, instalado: true, versao: '1', itens: [] }));
const config = (valor: boolean) => ({
  campos: { claude_statusline_update: { valor, definido: true, origem: 'app' } }, somente_leitura: {},
});
const opcoesCodex = (contexto = false, voz = false) => ({
  contexto_estendido: contexto, codex_voice_beta: voz, contexto_configurado: contexto ? 1000000 : null,
  compactacao: null, modelos: [{ model: 'gpt-6-astra', default: 272000, max: 872000 }],
});
let ler: (url: string, init?: RequestInit) => Promise<Response>;
let lerConfig: (init?: RequestInit) => Promise<Response>;
let lerOpcoesCodex: (init?: RequestInit) => Promise<Response>;
let componentes: ReturnType<typeof mount>[];

async function estabilizar() { for (let i = 0; i < 8; i++) await tick(); }
// A configuração vem do modal (o store), não de um GET desta tela: o dublê entrega o que o store
// já leu. `campos: {}` com `carregando: false` é o servidor que NÃO conhece a chave — é essa a
// diferença que faz o card avisar "opção indisponível" em vez de ficar mudo.
function dubleStore(campos: Record<string, unknown> = config(true).campos,
                    extra: { carregando?: boolean; erro?: string } = {}) {
  return {
    get campos() { return campos; }, get leitura() { return {}; },
    get carregando() { return extra.carregando ?? false; }, get salvando() { return false; },
    get erro() { return extra.erro ?? ''; }, get salvo() { return false; }, get temMudanca() { return false; },
    valorAtual: () => '', rascunhoDe: () => '', setRascunho: () => {},
    carregar: async () => {}, salvar: async () => {}, invalidar: () => {},
  } as never;
}
// Dublê REATIVO: o ↻ precisa ser observável (quantas vezes pediu leitura ao modal) e o dado do
// servidor precisa poder mudar por fora, que é o caso que o ↻ existe pra trazer.
function dubleStoreVivo(campos: Record<string, unknown> = config(true).campos, erro = '') {
  const st = $state({ campos, erro, cargas: 0 });
  const store = {
    get campos() { return st.campos; }, get leitura() { return {}; },
    get carregando() { return false; }, get salvando() { return false; },
    get erro() { return st.erro; }, get salvo() { return false; }, get temMudanca() { return false; },
    valorAtual: () => '', rascunhoDe: () => '', setRascunho: () => {},
    carregar: async () => { st.cargas++; st.erro = ''; }, salvar: async () => {}, invalidar: () => {},
  } as never;
  return { st, store };
}
async function montar(alvo: Server | null = B, store: unknown = dubleStore()) {
  const props = $state({ apiTarget: alvo, store: store as never });
  const el = document.createElement('div');
  document.body.appendChild(el);
  const comp = mount(HarnessSettings, { target: el, props });
  componentes.push(comp);
  await estabilizar();
  return { props, el, comp };
}
function botao(el: HTMLElement) {
  return [...el.querySelectorAll('button')].find((b) => b.textContent?.includes(m.harness_codex_reconciliar()))!;
}
function chamadasIntegracao() {
  return vi.mocked(fetch).mock.calls.filter(([url]) => String(url).endsWith(ROTA));
}

beforeEach(() => {
  vi.useFakeTimers();
  componentes = [];
  localStorage.setItem('cp_servers', JSON.stringify([A, B]));
  localStorage.setItem('cp_active', A.id);
  ler = async () => resposta(estado());
  lerConfig = async () => resposta(config(true));
  lerOpcoesCodex = async () => resposta(opcoesCodex());
  vi.spyOn(globalThis, 'fetch').mockImplementation(async (url, init) => {
    if (String(url).endsWith(ROTA)) return ler(String(url), init);
    if (String(url).endsWith(ROTA_INST)) return resposta(ociosa);
    if (String(url).endsWith(ROTA_CONFIG)) return lerConfig(init);
    if (String(url).endsWith(ROTA_CODEX)) return lerOpcoesCodex(init);
    return resposta(harnesses);
  });
});
afterEach(async () => {
  for (const comp of componentes) await unmount(comp);
  vi.useRealTimers(); vi.restoreAllMocks();
  localStorage.clear(); document.body.innerHTML = '';
});

describe('integração do Codex em Harnesses', () => {
  it('consulta e reconcilia na rota dedicada do servidor selecionado, sem polling ocioso', async () => {
    const { el } = await montar();
    expect(el.textContent).toContain(m.harness_codex_ocioso());
    await vi.advanceTimersByTimeAsync(10000);
    expect(chamadasIntegracao()).toHaveLength(1);
    botao(el).click(); await estabilizar();
    expect(chamadasIntegracao()).toHaveLength(2);
    expect(chamadasIntegracao()[1]).toEqual([
      `${B.baseUrl}${ROTA}`, expect.objectContaining({ method: 'POST', headers: { Authorization: `Bearer ${B.token}` } }),
    ]);
  });

  it('consulta o servidor ativo quando não há alvo explícito', async () => {
    await montar(null);
    expect(chamadasIntegracao()[0][0]).toBe(`${A.baseUrl}${ROTA}`);
  });

  it('acompanha operação já em andamento e para ao concluir, mostrando falhas e confiança', async () => {
    let n = 0;
    ler = async () => resposta(++n === 1 ? estado({ estado: 'executando' }) : estado({
      estado: 'parcial', ultima_execucao: '2026-09-06T15:00:00Z',
      plugins: [{ id: 'plugin-local', versao: '2.1', origem: 'marketplace-local' }],
      erros: ['Falha de importação'], avisos: ['Aviso de compatibilidade'], confianca_pendente: true,
    }));
    const { el } = await montar();
    expect([...el.querySelectorAll('button')].find((b) => b.textContent === m.harness_codex_executando())?.disabled).toBe(true);
    await vi.advanceTimersByTimeAsync(1500); await estabilizar();
    expect(el.textContent).toContain(m.harness_codex_parcial());
    expect(el.textContent).toContain('plugin-local');
    expect(el.textContent).toContain('marketplace-local');
    expect(el.textContent).toContain('Falha de importação');
    expect(el.textContent).toContain('Aviso de compatibilidade');
    expect(el.textContent).toContain(m.harness_codex_confianca());
    await vi.advanceTimersByTimeAsync(10000);
    expect(chamadasIntegracao()).toHaveLength(2);
    expect(chamadasIntegracao().every(([, init]) => !init?.method)).toBe(true);
  });

  it('não deixa GET antigo sobrescrever a operação iniciada pelo botão', async () => {
    let terminar!: (valor: Response) => void;
    ler = async (_url, init) => init?.method === 'POST'
      ? resposta(estado({ estado: 'executando' }))
      : new Promise((resolve) => { terminar = resolve; });
    const { el } = await montar();
    botao(el).click(); await estabilizar();
    terminar(resposta(estado())); await estabilizar();
    expect(el.textContent).toContain(m.harness_codex_executando());
    await vi.advanceTimersByTimeAsync(1500);
    expect(chamadasIntegracao()).toHaveLength(3);
  });

  it('aborta consulta anterior e descarta resposta atrasada ao trocar servidor', async () => {
    let terminar!: (valor: Response) => void;
    ler = async (url) => url.startsWith(B.baseUrl)
      ? new Promise((resolve) => { terminar = resolve; })
      : resposta(estado({ estado: 'ok' }));
    const { props, el } = await montar();
    const signal = chamadasIntegracao()[0][1]?.signal;
    props.apiTarget = A; await estabilizar();
    expect(signal?.aborted).toBe(true);
    terminar(resposta(estado({ estado: 'executando', erros: ['Resposta antiga'] })));
    await estabilizar(); await vi.advanceTimersByTimeAsync(5000);
    expect(el.textContent).toContain(m.harness_codex_ok());
    expect(el.textContent).not.toContain('Resposta antiga');
    expect(chamadasIntegracao()).toHaveLength(2);
  });

  it('desmontar interrompe o polling e aborta a consulta pendente', async () => {
    ler = async () => resposta(estado({ estado: 'executando' }));
    const { comp } = await montar();
    const signal = chamadasIntegracao()[0][1]?.signal;
    await unmount(comp); componentes = [];
    await vi.advanceTimersByTimeAsync(5000);
    expect(signal?.aborted).toBe(true);
    expect(chamadasIntegracao()).toHaveLength(1);
  });

  it('falha de integração mantém os demais harnesses e permite tentar novamente', async () => {
    ler = async () => { throw new Error('Servidor indisponível'); };
    const { el } = await montar();
    expect(el.textContent).toContain('claude');
    expect(el.textContent).toContain('Servidor indisponível');
    expect(botao(el).disabled).toBe(false);
    await vi.advanceTimersByTimeAsync(5000);
    expect(chamadasIntegracao()).toHaveLength(1);
  });

  it('permite reconciliar de novo quando perde conexão durante o polling', async () => {
    let n = 0;
    ler = async () => {
      if (++n === 2) throw new Error('Conexão interrompida');
      return resposta(estado({ estado: n === 1 ? 'executando' : 'ok' }));
    };
    const { el } = await montar();
    await vi.advanceTimersByTimeAsync(1500); await estabilizar();
    expect(el.textContent).toContain('Conexão interrompida');
    expect(botao(el).disabled).toBe(false);
    botao(el).click(); await estabilizar();
    expect(el.textContent).toContain(m.harness_codex_ok());
    expect(chamadasIntegracao()[2][1]?.method).toBe('POST');
  });

  it('mensagem com código é traduzida; código desconhecido e string crua mostram o texto', async () => {
    ler = async () => resposta(estado({
      estado: 'parcial',
      etapa: { codigo: 'etapa_pendencias', params: {}, texto: 'Confira os itens pendentes' },
      erros: [{ codigo: 'erro_plugin', params: { id: 'ecc@ecc' }, texto: 'Não foi possível reconciliar o plugin ecc@ecc' }],
      avisos: [{ codigo: 'inventado_no_futuro', params: {}, texto: 'Texto em pt do backend novo' }, 'String crua de backend antigo'],
      skills: { ponte: 42, nativas: 333 },
    }));
    const { el } = await montar();
    expect(el.textContent).toContain(m.harness_codex_m_etapa_pendencias());
    expect(el.textContent).toContain(m.harness_codex_m_erro_plugin({ id: 'ecc@ecc' }));
    expect(el.textContent).toContain('Texto em pt do backend novo');
    expect(el.textContent).toContain('String crua de backend antigo');
    expect(el.textContent).toContain(m.harness_codex_skills({ ponte: 42, nativas: 333 }));
  });

  it('o interruptor grava codex_sync no servidor e só muda depois da releitura', async () => {
    let automatica = true;
    ler = async () => resposta(estado({ automatica }));
    vi.mocked(fetch).mockImplementation(async (url, init) => {
      if (String(url).endsWith(ROTA_CONFIG)) {
        if (init?.method === 'POST') automatica = JSON.parse(String(init.body)).codex_sync;
        return resposta(config(true));
      }
      if (String(url).endsWith(ROTA_CODEX)) return lerOpcoesCodex(init);
      return String(url).endsWith(ROTA) ? ler(String(url), init) : resposta(harnesses);
    });
    const { el } = await montar();
    const caixa = el.querySelector<HTMLInputElement>('.hs-automatica input.switch')!;
    expect(caixa.checked).toBe(true);
    caixa.click(); await estabilizar();
    const gravacao = vi.mocked(fetch).mock.calls
      .find(([url, init]) => String(url).endsWith(ROTA_CONFIG) && init?.method === 'POST');
    expect(String(gravacao?.[0])).toContain(B.baseUrl);
    expect(JSON.parse(String(gravacao?.[1]?.body))).toEqual({ codex_sync: false });
    expect(caixa.checked).toBe(false);
    expect(chamadasIntegracao()).toHaveLength(2);
  });

  it('o "por quê?" de Reconciliar abre o motivo, e ele nasce recolhido', async () => {
    const { el } = await montar();
    const d = [...el.querySelectorAll('details.cfg-porque')].find(
      (x) => x.querySelector('summary')?.textContent?.includes(m.harness_codex_reconciliar_vered()),
    ) as HTMLDetailsElement | undefined;
    expect(d).toBeDefined();
    expect(d!.open).toBe(false);
    expect(d!.querySelector('summary')!.textContent).toContain(m.config_motores_por_que());
    expect(d!.querySelector('.cfg-motivo')!.textContent).toBe(m.harness_codex_reconciliar_porque());
    // Abrir é do elemento nativo: o teste exercita o atributo, que é o que o clique do navegador
    // troca — happy-dom não roda o comportamento padrão do <summary>.
    d!.open = true;
    expect(d!.open).toBe(true);
    // E mora no card do Codex, junto do botão que ele explica.
    expect(d!.closest('.hs-integracao')).not.toBeNull();
  });

  it('linha de item só ganha o "por quê?" quando a explicação existe pra ela', async () => {
    vi.mocked(fetch).mockImplementation(async (url, init) => {
      if (String(url).endsWith(ROTA)) return ler(String(url), init);
      if (String(url).endsWith(ROTA_INST)) return resposta(ociosa);
      if (String(url).endsWith(ROTA_CONFIG)) return lerConfig(init);
      if (String(url).endsWith(ROTA_CODEX)) return lerOpcoesCodex(init);
      // Os DOIS cards que têm o item `hooks`: no Kimi a linha são os avisos que o CLI manda pro
      // app; no Codex são os hooks do próprio usuário importados, e a mesma frase seria falsa.
      return resposta([
        { id: 'kimi', nome: 'Kimi', instalado: true, versao: '1', itens: [
          { id: 'hooks', codigo: 'hooks_nenhum', ok: false, params: {}, conserto: null },
          { id: 'mcp', codigo: 'mcp_nenhum', ok: true, params: {}, conserto: null },
        ] },
        { id: 'codex', nome: 'Codex', instalado: true, versao: '1', itens: [
          { id: 'hooks', codigo: 'hooks_codex', ok: true, info: true, params: { n: '17', eventos: 'Stop' }, conserto: null },
        ] },
      ]);
    });
    const { el } = await montar();
    /** [card, rótulo da linha, ganhou o bloco?] — o par é o que decide, não o id sozinho. */
    const explicadas = [...el.querySelectorAll('.hs-card')].flatMap((card) =>
      [...card.querySelectorAll('.hs-item')].map((linha) => [
        card.querySelector('.hs-nome')?.textContent,
        linha.querySelector('b')?.textContent,
        linha.nextElementSibling?.classList.contains('cfg-porque') ?? false,
      ]),
    );
    expect(explicadas).toContainEqual(['Kimi', m.harness_item_hooks(), true]);
    // O furo da rodada 1: o mesmo id, no card onde a frase não vale.
    expect(explicadas).toContainEqual(['Codex', m.harness_item_hooks(), false]);
    // Controle de "id sem entrada nenhuma".
    expect(explicadas).toContainEqual(['Kimi', m.harness_item_mcp(), false]);
    expect(el.textContent).toContain(m.harness_item_hooks_porque());
  });
});

// Vinham das folhas "Opções" (uma por harness), que deixaram de existir: o que elas mostravam e
// gravavam agora é conteúdo do card.
describe('opções dentro do card', () => {
  const claude = () => document.querySelector<HTMLInputElement>('#claude-statusline-update')!;
  const contexto = () => document.querySelector<HTMLInputElement>('#codex-contexto-estendido')!;
  const voz = () => document.querySelector<HTMLInputElement>('#codex-voice-beta')!;
  const posts = (rota: string) => vi.mocked(fetch).mock.calls
    .filter(([url, init]) => String(url).endsWith(rota) && init?.method === 'POST');

  it('não há mais botão "Opções" em card nenhum, e o ↻ mostra o texto "Recarregar"', async () => {
    const { el } = await montar();
    expect([...el.querySelectorAll('button')].map((b) => b.textContent?.trim()))
      .not.toContain(m.sessao_opcoes());
    expect(el.querySelector('.hs-refresh')!.textContent).toContain(m.arq_recarregar());
  });

  it('a barra de status do Claude grava na hora no servidor escolhido e só muda depois da releitura', async () => {
    let valor = true;
    lerConfig = async (init) => {
      if (init?.method === 'POST') valor = JSON.parse(String(init.body)).claude_statusline_update;
      return resposta(config(valor));
    };
    await montar();
    expect(claude().checked).toBe(true);
    claude().click(); await estabilizar();
    expect(posts(ROTA_CONFIG)).toHaveLength(1);
    expect(String(posts(ROTA_CONFIG)[0][0])).toContain(B.baseUrl);
    expect(JSON.parse(String(posts(ROTA_CONFIG)[0][1]?.body))).toEqual({ claude_statusline_update: false });
    expect(claude().checked).toBe(false);
  });

  it('erro ao gravar a barra de status aparece e o interruptor volta ao dado do servidor', async () => {
    lerConfig = async (init) => {
      if (init?.method === 'POST') throw new Error('Falha de conexão');
      return resposta(config(true));
    };
    await montar();
    claude().click(); await estabilizar();
    expect(document.body.textContent).toContain('Falha de conexão');
    expect(claude().checked).toBe(true);
  });

  it('servidor que não conhece a chave do Claude diz que a opção está indisponível, sem interruptor', async () => {
    const { el } = await montar(B, dubleStore({}));
    expect(el.textContent).toContain(m.harness_opcoes_indisponiveis());
    expect(claude()).toBeNull();
  });

  it('configuração ainda carregando não vira "indisponível" — nem interruptor, nem aviso', async () => {
    const { el } = await montar(B, dubleStore({}, { carregando: true }));
    expect(el.textContent).not.toContain(m.harness_opcoes_indisponiveis());
    expect(claude()).toBeNull();
  });

  it('leitura de configuração que falhou aparece como erro, não como opção indisponível', async () => {
    const { el } = await montar(B, dubleStore({}, { erro: 'Falha de conexão' }));
    expect(el.textContent).toContain('Falha de conexão');
    expect(el.textContent).not.toContain(m.harness_opcoes_indisponiveis());
  });

  it('o ↻ é a saída de uma leitura de configuração que falhou: pede a releitura ao modal', async () => {
    const { st, store } = dubleStoreVivo(config(true).campos, 'Falha de leitura');
    const { el } = await montar(B, store);
    expect(el.textContent).toContain('Falha de leitura');
    expect(claude()).toBeNull();
    el.querySelector<HTMLButtonElement>('.hs-refresh')!.click(); await estabilizar();
    expect(st.cargas).toBe(1);
    expect(el.textContent).not.toContain('Falha de leitura');
    expect(claude().checked).toBe(true);
  });

  it('depois de gravar, o ↻ mostra o que o servidor tem agora, não a última pintura da gravação', async () => {
    lerConfig = async (init) => resposta(config(init?.method !== 'POST'));
    const { st, store } = dubleStoreVivo();
    await montar(B, store);
    claude().click(); await estabilizar();
    expect(claude().checked).toBe(false);
    // Mudou por fora (outra aba, o terminal): o modal releria isso, a pintura da gravação não.
    st.campos = config(true).campos;
    document.querySelector<HTMLButtonElement>('.hs-refresh')!.click(); await estabilizar();
    expect(claude().checked).toBe(true);
  });

  it('a tela NÃO lê a configuração por conta própria: só o que o modal já leu', async () => {
    await montar();
    expect(vi.mocked(fetch).mock.calls.filter(([url, init]) =>
      String(url).endsWith(ROTA_CONFIG) && init?.method !== 'POST')).toHaveLength(0);
    expect(claude().checked).toBe(true);
  });

  it('contexto estendido e voz beta gravam na hora pelo endpoint do Codex, avisando o chat', async () => {
    let dado = opcoesCodex();
    const mudou = vi.fn();
    window.addEventListener('hangar:codex-voice-config', mudou);
    lerOpcoesCodex = async (init) => {
      if (init?.method === 'POST') dado = { ...dado, ...JSON.parse(String(init.body)) };
      return resposta(dado);
    };
    const { el } = await montar();
    expect(el.textContent).toContain(m.codex_contexto_novas());
    expect(el.textContent).toContain('gpt-6-astra');
    expect(el.textContent).toContain(m.comum_beta());
    expect(contexto().checked).toBe(false);

    contexto().click(); await estabilizar();
    expect(JSON.parse(String(posts(ROTA_CODEX)[0][1]?.body)))
      .toEqual({ contexto_estendido: true, codex_voice_beta: false });
    expect(contexto().checked).toBe(true);

    voz().click(); await estabilizar();
    expect(JSON.parse(String(posts(ROTA_CODEX)[1][1]?.body)))
      .toEqual({ contexto_estendido: true, codex_voice_beta: true });
    expect(voz().checked).toBe(true);
    expect(mudou).toHaveBeenCalledTimes(2);
    expect((mudou.mock.calls[1][0] as CustomEvent).detail).toEqual({ serverId: B.id, enabled: true });
    window.removeEventListener('hangar:codex-voice-config', mudou);
  });

  it('sem catálogo de modelos, diz isso em vez de uma lista vazia', async () => {
    lerOpcoesCodex = async () => resposta({ ...opcoesCodex(), modelos: [] });
    const { el } = await montar();
    expect(el.textContent).toContain(m.codex_contexto_sem_catalogo());
    expect(el.textContent).not.toContain(m.codex_contexto_limites());
  });

  it('Codex ausente não consulta as opções dele', async () => {
    vi.mocked(fetch).mockImplementation(async (url, init) => {
      if (String(url).endsWith(ROTA)) return ler(String(url), init);
      if (String(url).endsWith(ROTA_INST)) return resposta(ociosa);
      if (String(url).endsWith(ROTA_CONFIG)) return lerConfig(init);
      if (String(url).endsWith(ROTA_CODEX)) return lerOpcoesCodex(init);
      return resposta([{ id: 'codex', nome: 'codex', instalado: false, versao: null, itens: [] }]);
    });
    await montar();
    expect(vi.mocked(fetch).mock.calls.filter(([url]) => String(url).endsWith(ROTA_CODEX))).toHaveLength(0);
    expect(contexto()).toBeNull();
  });

  it('Recarregar relê as opções do Codex mesmo com a LISTA de harnesses fora do ar', async () => {
    // A folha apagada tinha "tentar de novo" próprio; o ↻ só o substitui se não ficar preso a um
    // endpoint que não é o das opções.
    const { el } = await montar();
    expect(contexto().checked).toBe(false);
    const antes = vi.mocked(fetch).mock.calls.filter(([url]) => String(url).endsWith(ROTA_CODEX)).length;

    lerOpcoesCodex = async () => resposta(opcoesCodex(true));
    vi.mocked(fetch).mockImplementation(async (url, init) => {
      if (String(url).endsWith(ROTA)) return ler(String(url), init);
      if (String(url).endsWith(ROTA_INST)) return resposta(ociosa);
      if (String(url).endsWith(ROTA_CONFIG)) return lerConfig(init);
      if (String(url).endsWith(ROTA_CODEX)) return lerOpcoesCodex(init);
      throw new Error('Lista fora do ar');
    });
    el.querySelector<HTMLButtonElement>('.hs-refresh')!.click(); await estabilizar();

    expect(el.textContent).toContain('Lista fora do ar');
    expect(vi.mocked(fetch).mock.calls.filter(([url]) => String(url).endsWith(ROTA_CODEX)).length).toBe(antes + 1);
    expect(contexto().checked).toBe(true);
  });

  // As duas abaixo guardavam a corrida entre a leitura PRÓPRIA desta tela e a gravação. A leitura
  // saiu (vem do modal); o que sobra guardar é a gravação atravessando a troca de alvo.
  it('trocar de servidor descarta o que a gravação pintou e volta ao dado do modal', async () => {
    lerConfig = async (init) => resposta(config(init?.method !== 'POST'));
    const { props } = await montar();
    expect(claude().checked).toBe(true);
    claude().click(); await estabilizar();
    expect(claude().checked).toBe(false);
    props.apiTarget = A; await estabilizar();
    expect(claude().checked).toBe(true);
  });

  it('gravação atrasada do servidor anterior não pinta o alvo novo', async () => {
    let terminar!: (r: Response) => void;
    lerConfig = async (init) => init?.method === 'POST'
      ? new Promise((resolve) => { terminar = resolve; })
      : resposta(config(true));
    const { props } = await montar();
    claude().click(); await estabilizar();
    props.apiTarget = A; await estabilizar();
    terminar(resposta(config(false))); await estabilizar();
    expect(claude().checked).toBe(true);
  });

  it('resposta atrasada do Codex não pinta o servidor novo', async () => {
    let terminar!: (r: Response) => void;
    lerOpcoesCodex = async (init) => init?.method === 'POST'
      ? new Promise((resolve) => { terminar = resolve; })
      : resposta(opcoesCodex());
    const { props } = await montar();
    contexto().click(); await estabilizar();

    lerOpcoesCodex = async () => resposta(opcoesCodex(false, true));
    props.apiTarget = A; await estabilizar();
    terminar(resposta(opcoesCodex(true))); await estabilizar();
    expect(contexto().checked).toBe(false);
    expect(voz().checked).toBe(true);
  });

  // As três travas de gravação são soltas num `finally`. Sem `if (consulta === ctx)` ali, a resposta
  // atrasada do servidor ANTERIOR destrava um interruptor cuja gravação no servidor NOVO ainda está
  // em voo — e o único roteiro que mostra isso tem um segundo clique, no servidor novo, antes de a
  // resposta do anterior chegar.
  it('resposta atrasada do Codex não destrava gravação em voo no servidor novo', async () => {
    let terminar!: (r: Response) => void;
    lerOpcoesCodex = async (init) => init?.method === 'POST'
      ? new Promise((resolve) => { terminar = resolve; })
      : resposta(opcoesCodex());
    const { props } = await montar();
    contexto().click(); await estabilizar();        // POST de B em voo

    lerOpcoesCodex = async (init) => init?.method === 'POST'
      ? new Promise(() => {})
      : resposta(opcoesCodex());
    props.apiTarget = A; await estabilizar();
    contexto().click(); await estabilizar();        // POST de A em voo
    expect(contexto().disabled).toBe(true);

    terminar(resposta(opcoesCodex(true))); await estabilizar();
    expect(contexto().disabled).toBe(true);
  });

  it('resposta atrasada da barra de status não destrava gravação em voo no servidor novo', async () => {
    let terminar!: (r: Response) => void;
    lerConfig = async (init) => init?.method === 'POST'
      ? new Promise((resolve) => { terminar = resolve; })
      : resposta(config(true));
    const { props } = await montar();
    claude().click(); await estabilizar();          // POST de B em voo

    lerConfig = async (init) => init?.method === 'POST'
      ? new Promise(() => {})
      : resposta(config(true));
    props.apiTarget = A; await estabilizar();
    claude().click(); await estabilizar();          // POST de A em voo
    expect(claude().disabled).toBe(true);

    terminar(resposta(config(false))); await estabilizar();
    expect(claude().disabled).toBe(true);
  });

  it('resposta atrasada da sincronização automática não destrava gravação em voo no servidor novo', async () => {
    const automatica = () => document.querySelector<HTMLInputElement>('.hs-automatica input.switch')!;
    let terminar!: (r: Response) => void;
    const so = (init?: RequestInit) => init?.method === 'POST';
    lerConfig = async (init) => so(init) ? new Promise((resolve) => { terminar = resolve; }) : resposta(config(true));
    const { props } = await montar();
    automatica().click(); await estabilizar();      // POST de B em voo

    lerConfig = async (init) => so(init) ? new Promise(() => {}) : resposta(config(true));
    props.apiTarget = A; await estabilizar();
    automatica().click(); await estabilizar();      // POST de A em voo
    expect(automatica().disabled).toBe(true);

    terminar(resposta(config(true))); await estabilizar();
    expect(automatica().disabled).toBe(true);
  });
});
