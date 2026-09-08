// @vitest-environment happy-dom
// A folha de "+ Adicionar" abre num passo ZERO: o que você quer adicionar? Sem ele, o catálogo de
// provedores respondia sozinho uma pergunta que nunca foi feita — e "modelo pro Claude Code", que é
// um dos três usos, não era alcançável por porta nenhuma da interface.
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { mount, unmount, tick } from 'svelte';
import NovaCredencialSheet from './NovaCredencialSheet.svelte';
import * as m from '../../paraglide/messages';

vi.mock('@hangar/core', async (importOriginal) => ({
  ...(await importOriginal<typeof import('@hangar/core')>()),
  criarConta: vi.fn(async () => ({ path: '/x', label: 'x', active: false })),
  putEngine: vi.fn(async () => ({ motores: {} })),
  putEngineForServer: vi.fn(async () => ({ motores: {} })),
  engineModelos: vi.fn(async () => ({ modelos: [] })),
  engineModelosForServer: vi.fn(async () => ({ modelos: [] })),
}));
vi.mock('../../lib/credenciais', () => ({
  sincronizarNosAgentes: vi.fn(async () => ({ resultado: { pi: { ok: true, motivo: '' } } })),
  codexLoginIniciar: vi.fn(async () => ({ etapa: 'aguardando', url: 'https://x', user_code: 'ABC' })),
  codexLoginPasso: vi.fn(async () => ({ etapa: 'aguardando', url: 'https://x', user_code: 'ABC' })),
  codexLoginCancelar: vi.fn(async () => ({ ok: true })),
}));

const onFechar = vi.fn();
const onCriada = vi.fn();

function montar() {
  const el = document.createElement('div');
  document.body.appendChild(el);
  const comp = mount(NovaCredencialSheet, {
    target: el,
    props: { apiTarget: null, onFechar, onCriada },
  });
  return { el, comp: comp as never, fim: () => { unmount(comp as never); el.remove(); } };
}

// O botão é achado pelo TEXTO da linha que ele fecha — a folha não tem id por item, e casar por
// posição quebraria a cada linha nova do catálogo.
function conectarDe(rotulo: string): HTMLButtonElement {
  const item = [...document.body.querySelectorAll<HTMLElement>('.nc-item')]
    .find((i) => i.textContent?.includes(rotulo));
  expect(item, `linha "${rotulo}" não está na tela`).toBeTruthy();
  return item!.querySelector<HTMLButtonElement>('.nc-conectar')!;
}

beforeEach(() => vi.clearAllMocks());

describe('NovaCredencialSheet — o passo "o quê"', () => {
  it('abre perguntando o quê, com as três opções e SEM o catálogo de provedores', async () => {
    const t = montar();
    await tick();
    expect(document.body.textContent).toContain(m.contas_nova_escolha());
    expect(document.body.textContent).toContain(m.contas_add_conta());
    expect(document.body.textContent).toContain(m.contas_add_modelo());
    expect(document.body.textContent).toContain(m.contas_add_chave());
    expect(document.body.querySelectorAll('.nc-item').length).toBe(3);
    // O catálogo responderia a pergunta antes de ela ser feita.
    expect(document.body.textContent).not.toContain(m.novacred_kimi_desc());
    expect(document.body.textContent).not.toContain('OpenCode Zen');
    t.fim();
  });

  it('as três opções têm ícone de traço, não as INICIAIS do rótulo', async () => {
    // "MO"/"CH" num quadradinho lê como placeholder que ninguém terminou: iniciais servem pra
    // distinguir MARCA numa lista de provedores, e escolha de caminho não é marca nenhuma.
    const t = montar();
    await tick();
    for (const item of document.body.querySelectorAll<HTMLElement>('.nc-item')) {
      expect(item.querySelector('svg')).not.toBeNull();
      expect(item.textContent).not.toMatch(/\b[A-Z]{2}\b/);
    }
    t.fim();
  });

  it('"Conta do Claude" vai DIRETO ao nome da conta, sem passar pelo catálogo', async () => {
    const t = montar();
    await tick();
    conectarDe(m.contas_add_conta()).click();
    await tick();
    expect(document.body.querySelector('.nc-lista')).toBeNull();
    expect(document.body.textContent).toContain(m.novacred_nome_conta());
    t.fim();
  });

  it('"Modelo pro Claude Code" mostra o catálogo sem as linhas de login e com o OmniRoute', async () => {
    const t = montar();
    await tick();
    conectarDe(m.contas_add_modelo()).click();
    await tick();
    expect(document.body.querySelector('.nc-lista')).not.toBeNull();
    expect(document.body.textContent).toContain('OmniRoute');
    expect(document.body.textContent).toContain('Kimi Code');
    // Entrar numa conta não é cadastrar modelo: as duas linhas de login ficam fora daqui.
    expect(document.body.textContent).not.toContain(m.novacred_claude_desc());
    expect(document.body.textContent).not.toContain(m.novacred_codex_desc());
    t.fim();
  });

  it('"Modelo pro Claude Code" + provedor abre o formulário COMPLETO do motor, já com a URL', async () => {
    const t = montar();
    await tick();
    conectarDe(m.contas_add_modelo()).click();
    await tick();
    conectarDe('Kimi Code').click();
    await tick();
    // O formulário curto de chave não serve aqui: sem modelo, janela e avançado, o motor nasce
    // incompleto e a sessão compacta a 200k.
    expect(document.body.querySelector('.mf')).not.toBeNull();
    expect(document.body.textContent).toContain(m.config_motores_avancado());
    expect(document.body.querySelector<HTMLInputElement>('input[name="base_url"]')!.value)
      .toBe('https://api.kimi.com/coding');
    t.fim();
  });

  it('"Chave pra outro agente" mostra o catálogo COM a linha do Codex/ChatGPT', async () => {
    const t = montar();
    await tick();
    conectarDe(m.contas_add_chave()).click();
    await tick();
    expect(document.body.textContent).toContain(m.novacred_codex_nome());
    // E a conta do Claude não é chave de agente nenhum.
    expect(document.body.textContent).not.toContain(m.novacred_claude_desc());
    t.fim();
  });

  it('voltar do catálogo devolve ao passo "o quê" — não fecha a folha', async () => {
    const t = montar();
    await tick();
    conectarDe(m.contas_add_chave()).click();
    await tick();
    document.body.querySelector<HTMLButtonElement>('.nc-voltar')!.click();
    await tick();
    expect(document.body.textContent).toContain(m.contas_nova_escolha());
    expect(document.body.querySelectorAll('.nc-item').length).toBe(3);
    expect(onFechar).not.toHaveBeenCalled();
    // No passo "o quê" não há um passo anterior — o ← some.
    expect(document.body.querySelector('.nc-voltar')).toBeNull();
    t.fim();
  });

  it('voltar do formulário devolve ao catálogo, um passo de cada vez', async () => {
    const t = montar();
    await tick();
    conectarDe(m.contas_add_chave()).click();
    await tick();
    conectarDe('Groq').click();
    await tick();
    document.body.querySelector<HTMLButtonElement>('.nc-voltar')!.click();
    await tick();
    expect(document.body.querySelector('.nc-lista')).not.toBeNull();
    expect(document.body.textContent).toContain(m.novacred_codex_nome());
    expect(onFechar).not.toHaveBeenCalled();
    t.fim();
  });
});
