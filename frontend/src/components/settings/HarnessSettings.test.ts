// @vitest-environment happy-dom
// O botão "Instalar" de um CLI ausente. A máquina de quem desenvolve tem todos os harnesses
// instalados, então o caminho que este teste cobre — card ausente, confirmação com o comando à
// mostra, progresso, releitura do disco no fim — não aparece abrindo a tela aqui.
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { mount, unmount, tick } from 'svelte';
import HarnessSettings from './HarnessSettings.svelte';
import * as m from '../../paraglide/messages';
import * as cred from '../../lib/credenciais';
import type { Harness, Instalacao } from '../../lib/credenciais';

vi.mock('../../lib/credenciais', () => ({
  listarHarnesses: vi.fn(),
  consertarHarness: vi.fn(),
  codexIntegracaoEstado: vi.fn(() => new Promise(() => {})),
  codexIntegracaoReconciliar: vi.fn(() => new Promise(() => {})),
  instalacaoEstado: vi.fn(),
  instalarHarness: vi.fn(),
}));
vi.mock('@hangar/core', async (importOriginal) => ({
  ...(await importOriginal<typeof import('@hangar/core')>()),
  patchConfig: vi.fn(), patchConfigForServer: vi.fn(),
}));

const c = vi.mocked(cred);

const CARD_AUSENTE: Harness = { id: 'kimi', nome: 'Kimi Code', instalado: false, versao: null, itens: [] };
const CARD_PRESENTE: Harness = { id: 'kimi', nome: 'Kimi Code', instalado: true, versao: '0.38.0', itens: [] };

function estado(over: Partial<Instalacao> = {}): Instalacao {
  return {
    fase: 'ocioso', harness: null, etapa: null, passo: 0, total: 4, log: [], ok: null, erro: null,
    comandos: { kimi: 'curl -fsSL https://code.kimi.com/kimi-code/install.sh | bash' },
    manual: { kimi: 'https://kimi.com/code', omp: 'https://github.com/can1357/oh-my-pi' },
    ...over,
  };
}

// Um poll de 1200ms fica agendado quando a fase é "rodando"; sem desmontar de verdade entre os
// casos, esse timer cai no caso seguinte com os mocks já zerados.
let montados: ReturnType<typeof mount>[] = [];

async function montar(cards: Harness[], inst: Instalacao) {
  c.listarHarnesses.mockResolvedValue(cards);
  c.instalacaoEstado.mockResolvedValue(inst);
  const el = document.createElement('div');
  document.body.appendChild(el);
  const comp = mount(HarnessSettings, { target: el, props: { apiTarget: null } });
  montados.push(comp);
  await tick(); await Promise.resolve(); await Promise.resolve(); await tick();
  return { el, comp: comp as never };
}

function botoes(el: HTMLElement): (string | undefined)[] {
  return [...el.querySelectorAll('button')].map((b) => b.textContent?.trim());
}

beforeEach(() => { vi.clearAllMocks(); montados = []; document.body.innerHTML = ''; });
afterEach(async () => { for (const comp of montados) await unmount(comp); document.body.innerHTML = ''; });

describe('HarnessSettings — instalar um CLI que falta', () => {
  it('card ausente com comando oferece o botão; instalado não oferece nada', async () => {
    const t = await montar([CARD_AUSENTE], estado());
    expect(botoes(t.el)).toContain(m.harness_inst_botao());
    unmount(t.comp);

    const u = await montar([CARD_PRESENTE], estado());
    expect(botoes(u.el)).not.toContain(m.harness_inst_botao());
    unmount(u.comp);
  });

  it('sem comando pra este sistema, mostra o link do fornecedor e nenhum botão', async () => {
    const t = await montar([CARD_AUSENTE], estado({ comandos: {} }));
    expect(botoes(t.el)).not.toContain(m.harness_inst_botao());
    expect(t.el.querySelector<HTMLAnchorElement>('.hs-link')!.href).toBe('https://kimi.com/code');
    unmount(t.comp);
  });

  it('a confirmação mostra o comando EXATO, e só o Instalar dela dispara', async () => {
    const t = await montar([CARD_AUSENTE], estado());
    t.el.querySelector<HTMLButtonElement>('.hs-btn')!.click();
    await tick();
    // O ModalDialog vive num portal pro <body>.
    const card = document.querySelector<HTMLElement>('.confirm-card')!;
    expect(card.querySelector('.hs-conf-cmd')!.textContent)
      .toBe('curl -fsSL https://code.kimi.com/kimi-code/install.sh | bash');
    expect(c.instalarHarness).not.toHaveBeenCalled();

    c.instalarHarness.mockResolvedValue(estado({ fase: 'rodando', harness: 'kimi', etapa: 'comando', passo: 1 }));
    card.querySelector<HTMLButtonElement>('.c-primary')!.click();
    await tick(); await Promise.resolve(); await Promise.resolve(); await tick();
    expect(c.instalarHarness).toHaveBeenCalledWith(null, 'kimi', expect.anything());
    expect(document.querySelector('.confirm-card')).toBeNull();
    unmount(t.comp);
  });

  it('cancelar fecha sem instalar nada', async () => {
    const t = await montar([CARD_AUSENTE], estado());
    t.el.querySelector<HTMLButtonElement>('.hs-btn')!.click();
    await tick();
    document.querySelector<HTMLElement>('.confirm-card')!.querySelector<HTMLButtonElement>('.c-btn')!.click();
    await tick();
    expect(document.querySelector('.confirm-card')).toBeNull();
    expect(c.instalarHarness).not.toHaveBeenCalled();
    unmount(t.comp);
  });

  it('rodando mostra a etapa e a saída do comando — nunca só um giro mudo', async () => {
    const t = await montar([CARD_AUSENTE], estado({
      fase: 'rodando', harness: 'kimi', etapa: 'comando', passo: 1,
      log: ['$ curl -fsSL https://code.kimi.com/kimi-code/install.sh | bash', 'baixando kimi 0.38.0'],
    }));
    expect(t.el.textContent).toContain(
      m.harness_inst_andamento({ passo: 1, total: 4, etapa: m.harness_inst_etapa_comando() }));
    expect(t.el.querySelector('.hs-inst-log')!.textContent).toContain('baixando kimi 0.38.0');
    unmount(t.comp);
  });

  it('falha mostra a etapa em que parou e o erro, e não relê o disco', async () => {
    c.listarHarnesses.mockClear();
    const t = await montar([CARD_AUSENTE], estado({
      fase: 'pronto', ok: false, harness: 'kimi', etapa: 'conferir',
      erro: 'o comando terminou, mas o CLI continua sem aparecer aqui',
    }));
    expect(t.el.textContent).toContain(
      m.harness_inst_falhou({ etapa: m.harness_inst_etapa_conferir() }));
    expect(t.el.querySelector('.hs-aviso.erro')!.textContent)
      .toContain('o comando terminou, mas o CLI continua sem aparecer aqui');
    // Uma leitura só (a da montagem): nada terminou DURANTE esta tela, então não há o que reler.
    expect(c.listarHarnesses).toHaveBeenCalledTimes(1);
    unmount(t.comp);
  });

  it('poll que falha no meio não congela a tela em "Instalando…" — reagenda', async () => {
    // O trabalho vive no servidor. Sem reagendar, `fase` ficava 'rodando' para sempre: o botão de
    // TODOS os cards ausentes desabilitado e o ↻ pulando a releitura, sem saída a não ser remontar.
    const t = await montar([CARD_AUSENTE], estado({ fase: 'rodando', harness: 'kimi', etapa: 'comando', passo: 1 }));
    c.instalacaoEstado.mockRejectedValueOnce(new Error('rede caiu'));
    await vi.waitFor(() => expect(t.el.textContent).toContain('rede caiu'), { timeout: 4000 });
    // E volta sozinha quando a rede volta.
    c.instalacaoEstado.mockResolvedValue(estado({ fase: 'pronto', ok: true, harness: 'kimi', etapa: 'ajustes' }));
    c.listarHarnesses.mockResolvedValue([CARD_PRESENTE]);
    await vi.waitFor(() => expect(t.el.textContent).toContain(m.harness_inst_pronto()), { timeout: 5000 });
  });

  it('erro que impede COMEÇAR fica na tela — não some no poll seguinte', async () => {
    // Limpando o erro no topo de cada chamada, o próprio poll de reparo apagava a mensagem 1,2s
    // depois: a pessoa clicava em Instalar e nada acontecia, sem uma palavra.
    const t = await montar([CARD_AUSENTE], estado());
    c.instalarHarness.mockRejectedValue(new Error('ja ha uma instalacao em curso (codex)'));
    t.el.querySelector<HTMLButtonElement>('.hs-btn')!.click();
    await tick();
    document.querySelector<HTMLElement>('.confirm-card')!.querySelector<HTMLButtonElement>('.c-primary')!.click();
    await vi.waitFor(() => expect(t.el.textContent).toContain('ja ha uma instalacao em curso'), { timeout: 3000 });
    // O poll de reparo roda em 1200ms e volta 'ocioso'; a mensagem tem de sobreviver a ele.
    await new Promise((r) => setTimeout(r, 2000));
    expect(t.el.textContent).toContain('ja ha uma instalacao em curso');
  });

  it('o erro aparece no card de QUEM falhou, não no da instalação anterior', async () => {
    const t = await montar(
      [{ ...CARD_AUSENTE, id: 'omp', nome: 'oh-my-pi' }, CARD_AUSENTE],
      estado({ fase: 'pronto', ok: true, harness: 'kimi', etapa: 'ajustes', comandos: { kimi: 'x', omp: 'y' } }),
    );
    c.instalarHarness.mockRejectedValue(new Error('BOOM-omp'));
    t.el.querySelector<HTMLButtonElement>('.hs-btn')!.click();   // o primeiro card é o omp
    await tick();
    document.querySelector<HTMLElement>('.confirm-card')!.querySelector<HTMLButtonElement>('.c-primary')!.click();
    await vi.waitFor(() => expect(t.el.textContent).toContain('BOOM-omp'), { timeout: 3000 });
    const cards = [...t.el.querySelectorAll('.hs-card')];
    expect(cards[0].textContent).toContain('BOOM-omp');
    expect(cards[1].textContent).not.toContain('BOOM-omp');
  });

  it('aviso de etapa pulada aparece junto do "pronto"', async () => {
    const t = await montar([CARD_PRESENTE], estado({
      fase: 'pronto', ok: true, harness: 'kimi', etapa: 'ajustes',
      avisos: ['a etapa do wrapper foi pulada: sem bash nesta máquina'],
    }));
    expect(t.el.textContent).toContain('a etapa do wrapper foi pulada');
  });

  it('com uma instalação em curso, o botão dos outros cards fica desabilitado', async () => {
    const t = await montar(
      [CARD_AUSENTE, { ...CARD_AUSENTE, id: 'omp', nome: 'oh-my-pi' }],
      estado({ fase: 'rodando', harness: 'kimi', etapa: 'comando', passo: 1, comandos: { kimi: 'x', omp: 'y' } }),
    );
    const insts = [...t.el.querySelectorAll('button')].filter((b) => b.textContent?.trim() === m.harness_inst_botao());
    expect(insts.length).toBe(2);
    expect(insts.every((b) => b.disabled)).toBe(true);
  });

  it('fase que o app não conhece não vira falha inventada', async () => {
    const t = await montar([CARD_AUSENTE], { ...estado(), fase: 'algo-novo' } as unknown as Instalacao);
    expect(t.el.textContent).not.toContain(m.harness_inst_falhou({ etapa: '' }).slice(0, 20));
    expect(t.el.querySelector('.hs-inst')).toBeNull();
  });

  it('link do fornecedor com esquema perigoso é recusado', async () => {
    const t = await montar([CARD_AUSENTE], estado({ comandos: {}, manual: { kimi: 'javascript:alert(1)' } }));
    expect(t.el.querySelector('.hs-link')).toBeNull();
  });

  it('resposta sem `log` não derruba a tela', async () => {
    // Backend mais velho que o app, ou resposta de outra forma: o painel inteiro não pode sumir
    // por causa de um campo ausente.
    const parcial = { fase: 'rodando', harness: 'kimi', etapa: 'comando', passo: 1, total: 4 };
    const t = await montar([CARD_AUSENTE], parcial as unknown as Instalacao);
    expect(t.el.querySelector('.hs-card')).not.toBeNull();
    expect(t.el.querySelector('.hs-inst-log')).toBeNull();
    unmount(t.comp);
  });

  it('quando a instalação TERMINA com a tela aberta, o card é relido do disco', async () => {
    const t = await montar([CARD_AUSENTE], estado({ fase: 'rodando', harness: 'kimi', etapa: 'ajustes', passo: 3 }));
    expect(c.listarHarnesses).toHaveBeenCalledTimes(1);
    // O poll seguinte encontra a instalação terminada: o card tem de ser relido do disco, senão
    // continuaria dizendo "não instalado" com o CLI já no lugar.
    c.instalacaoEstado.mockResolvedValue(estado({ fase: 'pronto', ok: true, harness: 'kimi', etapa: 'ajustes' }));
    c.listarHarnesses.mockResolvedValue([CARD_PRESENTE]);
    await vi.waitFor(() => {
      expect(t.el.textContent).toContain(m.harness_inst_pronto());
      expect(t.el.textContent).toContain('0.38.0');
    }, { timeout: 4000 });
    expect(c.listarHarnesses.mock.calls.length).toBeGreaterThan(1);
    unmount(t.comp);
  });
});
