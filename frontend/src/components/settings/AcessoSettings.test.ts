// @vitest-environment happy-dom
// Aba Acesso — pareamento (Task 6): o QR NÃO está no DOM antes do toque; depois do
// toque aparecem QR e o endereço em texto; há botão de esconder; trocar o endereço
// escolhido troca o conteúdo do QR.
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { mount, unmount, tick } from 'svelte';
import AcessoSettings from './AcessoSettings.svelte';
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
});
