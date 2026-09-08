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
let ler: (url: string, init?: RequestInit) => Promise<Response>;
let componentes: ReturnType<typeof mount>[];

async function estabilizar() { for (let i = 0; i < 8; i++) await tick(); }
async function montar(alvo: Server | null = B) {
  const props = $state({ apiTarget: alvo });
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
  vi.spyOn(globalThis, 'fetch').mockImplementation(async (url, init) => {
    if (String(url).endsWith(ROTA)) return ler(String(url), init);
    if (String(url).endsWith(ROTA_INST)) return resposta(ociosa);
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
      if (String(url).endsWith('/api/config')) {
        automatica = JSON.parse(String(init?.body)).codex_sync;
        return resposta({ campos: {} });
      }
      return String(url).endsWith(ROTA) ? ler(String(url), init) : resposta(harnesses);
    });
    const { el } = await montar();
    const caixa = el.querySelector<HTMLInputElement>('input.switch')!;
    expect(caixa.checked).toBe(true);
    caixa.click(); await estabilizar();
    const gravacao = vi.mocked(fetch).mock.calls.find(([url]) => String(url).endsWith('/api/config'));
    expect(String(gravacao?.[0])).toContain(B.baseUrl);
    expect(JSON.parse(String(gravacao?.[1]?.body))).toEqual({ codex_sync: false });
    expect(caixa.checked).toBe(false);
    expect(chamadasIntegracao()).toHaveLength(2);
  });
});
