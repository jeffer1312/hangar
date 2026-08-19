// @vitest-environment happy-dom
// Aba Acesso — pareamento (Task 6): o QR NÃO está no DOM antes do toque; depois do
// toque aparecem QR e o endereço em texto; há botão de esconder; trocar o endereço
// escolhido troca o conteúdo do QR.
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { mount, unmount, tick } from 'svelte';
import AcessoSettings from './AcessoSettings.svelte';
import { criarProps } from './props-reativas.svelte';
import * as m from '../../paraglide/messages';
import * as alcanceLib from '../../lib/alcance';
import * as auth from '../../lib/auth';
import type { Server } from '../../lib/auth';

vi.mock('../../lib/alcance', async (importOriginal) => {
  const real = await importOriginal<typeof import('../../lib/alcance')>();
  return {
    ...real,
    alcanceDoServidor: vi.fn(),
    pareamentoDoServidor: vi.fn(),
  };
});

// A tela resolve o servidor alvo pela rota (?srv=) ou pelo ativo. O SettingsModal é
// intocável, então o teste monta o componente direto e mocka o ativo (listServers +
// getActiveId) — a resolução cai no ativo.
const SRV: Server = { id: 'srv-a', label: 'A', baseUrl: 'http://a', token: 'x' } as Server;

vi.mock('../../lib/auth', async (importOriginal) => {
  const real = await importOriginal<typeof import('../../lib/auth')>();
  return {
    ...real,
    listServers: vi.fn(() => [SRV]),
    getActiveId: vi.fn(() => 'srv-a'),
  };
});

const alcanceMock = vi.mocked(alcanceLib);

function montar() {
  const el = document.createElement('div');
  document.body.appendChild(el);
  const comp = mount(AcessoSettings, { target: el });
  return { el, comp: comp as never };
}

// Lista de endereços padrão (mock estado 1): rede local ok, público não configurado.
function enderecosBasico() {
  return [
    { tipo: 'rede_local', url: 'http://192.168.0.42:5173', estado: 'ok', tempo_ms: 12 },
    { tipo: 'tailscale', url: 'https://hangar.tail9c2f.ts.net', estado: 'ok', tempo_ms: 84 },
    { tipo: 'publico', url: '', estado: 'nao_configurado', tempo_ms: null },
  ];
}

beforeEach(() => {
  vi.clearAllMocks();
  alcanceMock.alcanceDoServidor.mockResolvedValue({
    loopback: false,
    bind: '192.168.0.42',
    enderecos: enderecosBasico() as never,
  });
});

describe('AcessoSettings — pareamento', () => {
  it('QR e código NÃO estão no DOM antes do toque; o aviso e o botão sim', async () => {
    const t = montar();
    await tick(); await tick();
    expect(t.el.querySelector('.ac-qr')).toBeNull();
    expect(t.el.querySelector('.ac-cod')).toBeNull();
    expect(t.el.textContent).toContain(m.acesso_oculto_aviso());
    expect(t.el.textContent).toContain(m.acesso_mostrar_codigo());
    unmount(t.comp);
  });

  it('tocar em mostrar revela QR (img com o SVG) e o endereço em texto', async () => {
    alcanceMock.pareamentoDoServidor.mockResolvedValue({
      url: 'http://192.168.0.42:5173/?token=9f4c2ae1b73d08e5',
      qr_svg: '<svg xmlns="http://www.w3.org/2000/svg">QR</svg>',
    });
    const t = montar();
    await tick(); await tick();
    (t.el.querySelector('.ac-btn.primaria') as HTMLButtonElement).click();
    await tick(); await tick();
    // O QR chega pronto do backend como SVG; a tela o injeta inline no .ac-qr
    // (decisão de plano: backend desenha, front só exibe).
    const svg = t.el.querySelector<SVGSVGElement>('.ac-qr svg');
    expect(svg).not.toBeNull();
    expect(svg!.innerHTML).toContain('QR');
    expect(t.el.querySelector('.ac-cod')!.textContent).toBe('http://192.168.0.42:5173/?token=9f4c2ae1b73d08e5');
    unmount(t.comp);
  });

  it('há botão de esconder, e ele volta ao estado inicial (QR some do DOM)', async () => {
    alcanceMock.pareamentoDoServidor.mockResolvedValue({
      url: 'http://192.168.0.42:5173/?token=9f4c2ae1b73d08e5',
      qr_svg: '<svg xmlns="http://www.w3.org/2000/svg">QR</svg>',
    });
    const t = montar();
    await tick(); await tick();
    (t.el.querySelector('.ac-btn.primaria') as HTMLButtonElement).click();
    await tick(); await tick();
    const esconder = [...t.el.querySelectorAll('button')].find((b) => b.textContent === m.acesso_esconder())!;
    expect(esconder).toBeTruthy();
    esconder.click();
    await tick(); await tick();
    expect(t.el.querySelector('.ac-qr')).toBeNull();
    expect(t.el.textContent).toContain(m.acesso_mostrar_codigo());
    unmount(t.comp);
  });

  it('trocar o endereço escolhido chama a rota com o novo tipo e troca o QR', async () => {
    alcanceMock.pareamentoDoServidor.mockResolvedValueOnce({
      url: 'http://192.168.0.42:5173/?token=abc',
      qr_svg: '<svg>A</svg>',
    }).mockResolvedValueOnce({
      url: 'https://hangar.tail9c2f.ts.net/?token=abc',
      qr_svg: '<svg>B</svg>',
    });
    const t = montar();
    await tick(); await tick();
    (t.el.querySelector('.ac-btn.primaria') as HTMLButtonElement).click();
    await tick(); await tick();
    // Troca o endereço embutido para Tailscale (a escolha vem da lista de endereços).
    const select = t.el.querySelector<HTMLSelectElement>('.ac-par select')!;
    expect(select).not.toBeNull();
    select.value = 'tailscale';
    select.dispatchEvent(new Event('change', { bubbles: true }));
    await tick(); await tick();
    expect(alcanceMock.pareamentoDoServidor).toHaveBeenLastCalledWith(SRV, 'tailscale');
    const svg = t.el.querySelector<SVGSVGElement>('.ac-qr svg');
    expect(svg!.innerHTML).toContain('B');
    unmount(t.comp);
  });

  // ── Rodada 4 (bloqueador: ramo de ERRO concluía sem_candidato) ────────────────

  it('R4: lista que FALHOU não conclui sem_candidato — botão presente, desabilitado, frase de falha', async () => {
    // O servidor não respondeu: NENHUM endereço foi testado. A tela não pode dizer
    // "Nenhum endereço respondeu… Confira a lista e libere um endereço" — quem falhou
    // foi o servidor, não há endereço a liberar (bloqueador da rodada 3).
    alcanceMock.alcanceDoServidor.mockRejectedValue(
      new DOMException('signal timed out', 'TimeoutError'),
    );
    const t = montar();
    await tick(); await tick(); await tick();
    // A lista mostra a linha de falha.
    expect(t.el.textContent).toContain(m.falha_conexao());
    // O bloco NÃO afirma sem candidato; mostra o aviso de sempre com o botão DESABILITADO.
    expect(t.el.textContent).not.toContain(m.acesso_par_sem_candidato());
    const botao = t.el.querySelector<HTMLButtonElement>('.ac-btn.primaria');
    expect(botao).not.toBeNull();
    expect(botao!.disabled).toBe(true);
    unmount(t.comp);
  });

  // ── Rodada 3 (bloqueador único: conclusão antes de medir) ─────────────────────

  it('R3: enquanto a lista está em voo, NÃO afirma sem candidato e o botão fica desabilitado', async () => {
    // Promessa SEGURADA: a lista nunca resolve durante a primeira metade do teste —
    // é a janela em que o bloco de pareamento se contradizia ("Nenhum endereço
    // respondeu" com a lista acima em "Testando…").
    let resolver!: (v: never) => void;
    alcanceMock.alcanceDoServidor.mockReturnValue(new Promise((r) => { resolver = r; }) as never);
    const t = montar();
    await tick(); await tick();
    // Durante a carga: o botão de revelar existe mas está DESABILITADO (parTipo é ''),
    // e a frase de sem-candidato NÃO apareceu — conclusão só depois de medir.
    const botao = t.el.querySelector<HTMLButtonElement>('.ac-btn.primaria');
    expect(botao).not.toBeNull();
    expect(botao!.disabled).toBe(true);
    expect(t.el.textContent).not.toContain(m.acesso_par_sem_candidato());
    // Resolve a lista sem candidato: agora sim a tela conclui e o bloco troca.
    resolver({ loopback: false, bind: '192.168.0.42', enderecos: [] } as never);
    await tick(); await tick();
    expect(t.el.textContent).toContain(m.acesso_par_sem_candidato());
    expect(t.el.querySelector('.ac-btn.primaria')).toBeNull();
    unmount(t.comp);
  });

  it('R3: lista em voo que resolve COM candidato acende o botão (fica habilitado)', async () => {
    let resolver!: (v: never) => void;
    alcanceMock.alcanceDoServidor.mockReturnValue(new Promise((r) => { resolver = r; }) as never);
    const t = montar();
    await tick(); await tick();
    const botao = t.el.querySelector<HTMLButtonElement>('.ac-btn.primaria');
    expect(botao).not.toBeNull();
    expect(botao!.disabled).toBe(true);
    // Resolve COM candidato: o botão acende e o bloco permanece no aviso inicial.
    resolver({ loopback: false, bind: '192.168.0.42', enderecos: enderecosBasico() } as never);
    await tick(); await tick(); await tick();
    const botao2 = t.el.querySelector<HTMLButtonElement>('.ac-btn.primaria');
    expect(botao2).not.toBeNull();
    expect(botao2!.disabled).toBe(false);
    expect(t.el.textContent).not.toContain(m.acesso_par_sem_candidato());
    unmount(t.comp);
  });

  // ── Rodada 2 (bloqueadores 2-5) ───────────────────────────────────────────────

  it('B2: sem nenhum candidato ok, mostra estado nomeado e NENHUM botão de revelar', async () => {
    alcanceMock.alcanceDoServidor.mockResolvedValue({
      loopback: true,
      bind: '127.0.0.1',
      enderecos: [
        { tipo: 'nesta_maquina', url: 'http://127.0.0.1:5173', estado: 'ok', tempo_ms: 1 },
        { tipo: 'rede_local', url: 'http://192.168.15.117:5173', estado: 'falhou', tempo_ms: 1 },
        { tipo: 'publico', url: '', estado: 'nao_configurado', tempo_ms: null },
      ] as never,
    });
    const t = montar();
    await tick(); await tick();
    expect(t.el.textContent).toContain(m.acesso_par_sem_candidato());
    expect(t.el.querySelector('.ac-btn.primaria')).toBeNull();
    expect(t.el.querySelector('.ac-par')).toBeNull();
    unmount(t.comp);
  });

  it('B3: padrão é o candidato de MENOR tempo; a frase "mais rápido" some na escolha manual', async () => {
    // Tempos invertidos: tailscale 5 ms (mais rápido), rede_local 40 ms.
    alcanceMock.alcanceDoServidor.mockResolvedValue({
      loopback: false,
      bind: '192.168.0.42',
      enderecos: [
        { tipo: 'rede_local', url: 'http://192.168.0.42:5173', estado: 'ok', tempo_ms: 40 },
        { tipo: 'tailscale', url: 'https://hangar.tail9c2f.ts.net', estado: 'ok', tempo_ms: 5 },
        { tipo: 'publico', url: '', estado: 'nao_configurado', tempo_ms: null },
      ] as never,
    });
    alcanceMock.pareamentoDoServidor.mockResolvedValue({
      url: 'https://hangar.tail9c2f.ts.net/?token=abc',
      qr_svg: '<svg>QR</svg>',
    });
    const t = montar();
    await tick(); await tick(); await tick();
    // O padrão nasce no TAILSCALE (5 ms), não no primeiro da ordem do backend.
    const select = t.el.querySelector<HTMLSelectElement>('.ac-par select');
    expect(select).toBeNull(); // ainda escondido
    (t.el.querySelector('.ac-btn.primaria') as HTMLButtonElement).click();
    await tick(); await tick();
    const sel = t.el.querySelector<HTMLSelectElement>('.ac-par select')!;
    expect(sel.value).toBe('tailscale');
    expect(alcanceMock.pareamentoDoServidor).toHaveBeenLastCalledWith(SRV, 'tailscale');
    // A frase "respondeu mais rápido" aparece (a escolha é a automática).
    expect(t.el.textContent).toContain(m.acesso_par_escolhido({ rede: m.acesso_tailscale() }));
    // Escolha manual → rede_local: a frase SOME (mentiria).
    sel.value = 'rede_local';
    sel.dispatchEvent(new Event('change', { bubbles: true }));
    await tick(); await tick();
    expect(t.el.textContent).not.toContain(m.acesso_par_escolhido({ rede: m.acesso_rede_local() }));
    unmount(t.comp);
  });

  it('B4: erro de transporte mostra só a frase da casa, sem o quadrado do QR, e com Esconder', async () => {
    // Timeout de transporte: DOMException TimeoutError (o mesmo do AbortSignal.timeout).
    alcanceMock.pareamentoDoServidor.mockRejectedValue(
      new DOMException('signal timed out', 'TimeoutError'),
    );
    const t = montar();
    await tick(); await tick();
    (t.el.querySelector('.ac-btn.primaria') as HTMLButtonElement).click();
    await tick(); await tick();
    // Só a frase da casa, sem o "signal timed out" cru em inglês.
    expect(t.el.textContent).toContain(m.falha_conexao());
    expect(t.el.textContent).not.toContain('signal timed out');
    // Sem o retângulo branco do QR no erro.
    expect(t.el.querySelector('.ac-qr')).toBeNull();
    // Com botão Esconder (não é beco sem saída).
    const esconder = [...t.el.querySelectorAll('button')].find((b) => b.textContent === m.acesso_esconder());
    expect(esconder).toBeTruthy();
    unmount(t.comp);
  });

  it('B4: erro de API (400 traduzido) mostra o detalhe traduzido, com Esconder', async () => {
    alcanceMock.pareamentoDoServidor.mockRejectedValue(
      new Error('400: Credencial de pareamento não configurada — defina CP_AUTH_TOKEN para liberar o QR.'),
    );
    const t = montar();
    await tick(); await tick();
    (t.el.querySelector('.ac-btn.primaria') as HTMLButtonElement).click();
    await tick(); await tick();
    expect(t.el.textContent).toContain('Credencial de pareamento não configurada');
    expect(t.el.querySelector('.ac-qr')).toBeNull();
    const esconder = [...t.el.querySelectorAll('button')].find((b) => b.textContent === m.acesso_esconder());
    expect(esconder).toBeTruthy();
    unmount(t.comp);
  });

  it('B5: revelar leva o foco ao seletor; esconder devolve ao botão que abriu', async () => {
    alcanceMock.pareamentoDoServidor.mockResolvedValue({
      url: 'http://192.168.0.42:5173/?token=abc',
      qr_svg: '<svg>QR</svg>',
    });
    const t = montar();
    await tick(); await tick();
    const botao = t.el.querySelector<HTMLButtonElement>('.ac-btn.primaria')!;
    botao.focus();
    botao.click();
    await tick(); await tick(); await tick();
    // Foco foi para o seletor (não caiu no body).
    expect(document.activeElement?.classList.contains('ac-select')).toBe(true);
    // Esconder devolve o foco ao botão que abriu (o nó é recriado no DOM — compara
    // com o botão ATUAL, não com a referência antiga que foi desmontada).
    const esconder = [...t.el.querySelectorAll('button')].find((b) => b.textContent === m.acesso_esconder())!;
    esconder.click();
    await tick(); await tick(); await tick();
    const botaoAtual = t.el.querySelector<HTMLButtonElement>('.ac-btn.primaria')!;
    expect(botaoAtual).not.toBeNull();
    expect(document.activeElement).toBe(botaoAtual);
    unmount(t.comp);
  });

  it('B5: o nome acessível do seletor é só "Endereço no QR", sem a frase de aviso', async () => {
    alcanceMock.pareamentoDoServidor.mockResolvedValue({
      url: 'http://192.168.0.42:5173/?token=abc',
      qr_svg: '<svg>QR</svg>',
    });
    const t = montar();
    await tick(); await tick();
    (t.el.querySelector('.ac-btn.primaria') as HTMLButtonElement).click();
    await tick(); await tick();
    const sel = t.el.querySelector<HTMLSelectElement>('.ac-par select')!;
    const nome = sel.labels?.[0]?.textContent ?? '';
    // O label rotula o seletor com o texto próprio; o que NÃO pode é o aviso inteiro
    // ("O QR embute o endereço escolhido…") ter virado parte do nome.
    expect(nome).toContain(m.acesso_selecionar_endereco());
    expect(nome).not.toContain(m.acesso_par_trocar_aviso());
    unmount(t.comp);
  });
});

describe('AcessoSettings — troca de alvo com a tela montada (seletor do grupo)', () => {
  // Com o seletor de servidor no painel (19/08/2026) a troca NÃO remonta a tela: o SettingsModal
  // passa o alvo por prop e o efeito remede. Sem isto, endereços e QR do servidor anterior
  // ficavam na tela com o seletor dizendo o novo (achado da revisão).
  it('trocar a prop alvo remede os endereços e some com o dado do servidor anterior', async () => {
    const B: Server = { id: 'srv-b', label: 'B', baseUrl: 'http://b', token: 'y' } as Server;
    const props = criarProps({ alvo: SRV as Server | null });
    const el = document.createElement('div');
    document.body.appendChild(el);
    const comp = mount(AcessoSettings, { target: el, props });
    await tick(); await tick(); await tick();
    expect(alcanceMock.alcanceDoServidor).toHaveBeenCalledWith(SRV);
    expect(el.textContent).toContain('192.168.0.42');

    alcanceMock.alcanceDoServidor.mockResolvedValue({
      loopback: false, bind: '10.0.0.9',
      enderecos: [{ tipo: 'rede_local', url: 'http://10.0.0.9:5173', estado: 'ok', tempo_ms: 5 }] as never,
    });
    props.alvo = B;
    await tick(); await tick(); await tick();
    expect(alcanceMock.alcanceDoServidor).toHaveBeenCalledWith(B);
    expect(el.textContent).toContain('10.0.0.9');
    expect(el.textContent).not.toContain('192.168.0.42');
    unmount(comp as never);
  });
});
