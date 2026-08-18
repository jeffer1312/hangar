// @vitest-environment happy-dom
// ROTA de verdade, sem mock de biblioteca. É o único jeito de provar PARA ONDE a aba vai: a
// suíte que troca lib/contaEstado/lib/api/lib/loginConta por mocks prova que o botão chama a
// função, nunca o destino — foi exatamente por isso que o bloqueador da revisão final (a aba
// Contas agindo no servidor ATIVO com o cabeçalho nomeando outro) passou por três portões.
//
// O defeito: com ?srv=B e o ativo em A, listar/criar/apagar/Entrar falavam com A — apagar
// removia a pasta e os transcripts na máquina errada; Entrar colava a credencial OAuth no host
// errado. AcessoSettings é o CONTROLE: resolve o alvo sozinha pela rota e sempre acertou.
// Comportamento final: com ?srv=B a aba fala com B; sem ?srv= (alvo null) segue pelo caminho
// global — o servidor ativo — como sempre fez.
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { mount, unmount, tick } from 'svelte';
import ContasSettings from './ContasSettings.svelte';
import AcessoSettings from './AcessoSettings.svelte';
import type { Server } from '../../lib/auth';

const SRV_A: Server = { id: 'srv-a', label: 'A', baseUrl: 'http://a.local:8765', token: 't-a' };
const SRV_B: Server = { id: 'srv-b', label: 'B', baseUrl: 'http://b.local:8765', token: 't-b' };

function resposta(json: unknown) {
  return { ok: true, status: 200, json: async () => json } as Response;
}

const urls: string[] = [];

beforeEach(() => {
  urls.length = 0;
  localStorage.setItem('cp_servers', JSON.stringify([SRV_A, SRV_B]));
  localStorage.setItem('cp_active', 'srv-a');
  vi.spyOn(globalThis, 'fetch').mockImplementation(async (input: RequestInfo | URL) => {
    urls.push(String(input));
    return resposta(input.toString().includes('/api/alcance')
      ? { loopback: false, bind: 'x', enderecos: [] }
      : []);
  });
});
afterEach(() => {
  vi.restoreAllMocks();
  location.hash = '';
  localStorage.clear();
  document.body.innerHTML = '';
});

function montar(componente: unknown, props?: Record<string, unknown>) {
  const el = document.createElement('div');
  document.body.appendChild(el);
  const comp = mount(componente as never, { target: el, props: props as never });
  return { el, comp: comp as never };
}

describe('aba Contas fala com o servidor do ?srv=, não com o ativo', () => {
  it('?srv=srv-b com o ativo em srv-a: a lista de Contas sai para o baseUrl de B', async () => {
    location.hash = '#/?config=contas&srv=srv-b';
    const t = montar(ContasSettings, { apiTarget: SRV_B });
    await tick(); await tick(); await tick();
    expect(urls.filter((u) => u.includes('/api/credenciais'))[0])
      .toMatch(/^http:\/\/b\.local:8765\/api\/credenciais$/);
    unmount(t.comp);
  });

  it('sem ?srv= (alvo null) a lista sai para o ATIVO, como sempre', async () => {
    location.hash = '#/?config=contas';
    const t = montar(ContasSettings, { apiTarget: null });
    await tick(); await tick(); await tick();
    expect(urls.filter((u) => u.includes('/api/credenciais'))[0])
      .toMatch(/^http:\/\/a\.local:8765\/api\/credenciais$/);
    unmount(t.comp);
  });

  it('controle: AcessoSettings com ?srv=srv-b fala com B (resolve o alvo sozinha)', async () => {
    location.hash = '#/?config=acesso&srv=srv-b';
    const t = montar(AcessoSettings);
    await tick(); await tick(); await tick();
    expect(urls.filter((u) => u.includes('/api/alcance'))[0])
      .toMatch(/^http:\/\/b\.local:8765\/api\/alcance/);
    unmount(t.comp);
  });
});
