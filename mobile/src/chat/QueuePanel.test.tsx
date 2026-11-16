// @vitest-environment happy-dom
import { describe, expect, it, vi } from 'vitest';
import { act, createElement, type ReactNode } from 'react';
import { createRoot } from 'react-dom/client';
import { QueuePanel } from './QueuePanel';

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

// Cada chave devolve o próprio nome, como nos outros testes de componente do app.
vi.mock('../paraglide/messages', () => ({ ...Object.fromEntries(
  ('queue_panel_title queue_panel_collapse queue_panel_expand queue_panel_actions queue_panel_move_up queue_panel_move_down '
    + 'queue_panel_remove codex_orientar codex_orientar_ajuda atalhos_editar comum_apagar comum_cancelar '
    + 'problema_headless_delivery_stalled reabrir_sessao_presa chat_servidor_removido native_invalid_response erro_desconhecido')
    .split(' ').map((k) => [k, () => k]),
), queue_panel_steer_message: ({ text }: { text: string }) => `queue_panel_steer_message: ${text}`,
  erro_recarregar: ({ erro }: { erro: string }) => `erro_recarregar: ${erro}` }));
vi.mock('../ui/Icon', () => ({ Icon: () => null }));
vi.mock('../ui/Glass', () => ({ Glass: ({ children }: { children: ReactNode }) => createElement('div', null, children) }));
// O mock de react-native diz Android: sem isto o menu nativo de verdade seria carregado.
vi.mock('@react-native-menu/menu', () => ({ MenuView: ({ children }: { children: ReactNode }) => createElement('div', null, children) }));
const reopen = vi.hoisted(() => ({
  recarregar: vi.fn<(name: string, server: unknown) => Promise<{ ok: boolean }>>(),
  erro: vi.fn(), success: vi.fn(), announce: vi.fn(),
  servers: [{ id: 'a', label: 'a', baseUrl: 'http://10.0.0.1:8765', token: 'token-a' }],
}));
vi.mock('react-native', async (original) => {
  const native = await original<typeof import('react-native')>();
  return { ...native, AccessibilityInfo: { ...native.AccessibilityInfo, announceForAccessibility: reopen.announce } };
});
vi.mock('@hangar/core', async (original) => ({ ...await original<typeof import('@hangar/core')>(), recarregarSessao: reopen.recarregar }));
vi.mock('../stores/servers', () => ({ useServers: { getState: () => ({ servers: reopen.servers }) } }));
vi.mock('../ui/Toast', () => ({ toast: { erro: reopen.erro } }));
vi.mock('../ui/haptics', () => ({ haptic: { selection: vi.fn(), success: reopen.success, error: vi.fn() } }));

const panel = { actions: true, entries: [{ id: 'a', text: 'primeira', ts: 1 }, { id: 'b', text: 'segunda', ts: 2 }] };

function render(props: Record<string, unknown>) {
  const handlers = { onEdit: vi.fn(), onCancelEdit: vi.fn(), onSteer: vi.fn(), onRemove: vi.fn(), onMove: vi.fn() };
  const container = document.createElement('div');
  const root = createRoot(container);
  act(() => root.render(createElement(QueuePanel, { panel, editingId: null, busy: false, ...handlers, ...props } as never)));
  return { container, root, handlers };
}
const buttons = (container: HTMLElement, label: string) =>
  [...container.querySelectorAll<HTMLButtonElement>(`[aria-label="${label}"]`)];
const steerButtons = (container: HTMLElement) =>
  [...container.querySelectorAll<HTMLButtonElement>('[aria-label^="queue_panel_steer_message: "]')];

describe('QueuePanel (app)', () => {
  it('lista as mensagens e recolhe pelo cabeçalho', () => {
    const { container, root } = render({});
    expect(container.textContent).toContain('queue_panel_title');
    expect(container.textContent).toContain('primeira');
    act(() => buttons(container, 'queue_panel_collapse')[0].click());
    expect(container.textContent).not.toContain('primeira');
    expect(buttons(container, 'queue_panel_expand')).toHaveLength(1);
    act(() => root.unmount());
  });

  it('menu da linha tem Subir, Descer, Orientar, Editar e Apagar nessa ordem', () => {
    expect(QueuePanel.menuActions({ index: 0, count: 2, busy: false }).map((a) => a.id))
      .toEqual(['up', 'down', 'steer', 'edit', 'remove']);
    const first = QueuePanel.menuActions({ index: 0, count: 2, busy: false });
    expect(first.find((a) => a.id === 'up')?.disabled).toBe(true);
    expect(first.find((a) => a.id === 'down')?.disabled).toBe(false);
    expect(QueuePanel.menuActions({ index: 1, count: 2, busy: false }).find((a) => a.id === 'down')?.disabled).toBe(true);
    expect(QueuePanel.menuActions({ index: 1, count: 2, busy: true }).every((a) => a.disabled)).toBe(true);
    // A linha em edição só se move: o resto descartaria o que está no composer.
    const editing = QueuePanel.menuActions({ index: 0, count: 2, busy: false, editing: true });
    expect(editing.filter((a) => !a.disabled).map((a) => a.id)).toEqual(['down']);
  });

  it('Orientar e X agem na linha certa; ocupado, ficam desativados', () => {
    const { container, root, handlers } = render({});
    expect(steerButtons(container).map((b) => b.getAttribute('aria-label'))).toEqual([
      'queue_panel_steer_message: primeira', 'queue_panel_steer_message: segunda',
    ]);
    act(() => buttons(container, 'queue_panel_steer_message: segunda')[0].click());
    expect(handlers.onSteer).toHaveBeenCalledWith('b');
    act(() => buttons(container, 'queue_panel_remove')[0].click());
    expect(handlers.onRemove).toHaveBeenCalledWith('a');
    act(() => root.render(createElement(QueuePanel, { panel, editingId: null, busy: true, ...handlers })));
    expect(steerButtons(container)).toHaveLength(2);
    expect([...steerButtons(container), ...buttons(container, 'queue_panel_remove')].every((b) => b.disabled)).toBe(true);
    act(() => root.unmount());
  });

  it('rótulo de Orientar normaliza o trecho e o limita a 80 caracteres', () => {
    const { container, root } = render({ panel: { actions: true, entries: [
      { id: 'a', text: ` \n a\t ${'b'.repeat(90)} fim `, ts: 1 },
    ] } });
    expect(steerButtons(container).map((b) => b.getAttribute('aria-label')))
      .toEqual([`queue_panel_steer_message: a ${'b'.repeat(78)}`]);
    act(() => root.unmount());
  });

  it('linha em edição troca Orientar e X por Cancelar', () => {
    const { container, root, handlers } = render({ editingId: 'a' });
    expect(steerButtons(container)).toHaveLength(1);
    expect(buttons(container, 'queue_panel_steer_message: primeira')).toHaveLength(0);
    expect(buttons(container, 'queue_panel_steer_message: segunda')).toHaveLength(1);
    expect(buttons(container, 'queue_panel_remove')).toHaveLength(1);
    const cancel = [...container.querySelectorAll('button')].find((b) => b.textContent === 'comum_cancelar')!;
    act(() => cancel.click());
    expect(handlers.onCancelEdit).toHaveBeenCalled();
    act(() => root.unmount());
  });

  it('sem o aviso de entrega parada: nem a frase nem o Reabrir', () => {
    const { container, root } = render({});
    expect(container.textContent).not.toContain('problema_headless_delivery_stalled');
    expect(buttons(container, 'reabrir_sessao_presa')).toHaveLength(0);
    act(() => root.unmount());
  });

  it('entrega parada: frase e Reabrir no painel; o pedido vai ao servidor da sessão e o botão espera a resposta', async () => {
    reopen.recarregar.mockReset();
    reopen.erro.mockClear();
    reopen.success.mockClear();
    reopen.announce.mockClear();
    let finish!: (ok: boolean) => void;
    reopen.recarregar.mockImplementation(() => new Promise((done, fail) => {
      finish = (ok) => (ok ? done({ ok: true }) : fail(new Error('reabrir recusado')));
    }));
    const { container, root } = render({ reopen: { serverId: 'a', name: 'sess' } });
    expect(container.textContent).toContain('primeira');
    expect(container.textContent).toContain('problema_headless_delivery_stalled');
    // O VoiceOver não anuncia `accessibilityRole="alert"` num Text: o aviso é falado ao aparecer.
    expect(reopen.announce).toHaveBeenCalledExactlyOnceWith('problema_headless_delivery_stalled');
    act(() => buttons(container, 'reabrir_sessao_presa')[0].click());
    expect(buttons(container, 'reabrir_sessao_presa')[0].disabled).toBe(true);
    act(() => buttons(container, 'reabrir_sessao_presa')[0].click());
    expect(reopen.recarregar).toHaveBeenCalledExactlyOnceWith('sess', reopen.servers[0]);
    await act(async () => finish(false));
    expect(reopen.erro).toHaveBeenCalledExactlyOnceWith('erro_recarregar: reabrir recusado');
    expect(reopen.success).not.toHaveBeenCalled();
    expect(buttons(container, 'reabrir_sessao_presa')[0].disabled).toBe(false);
    act(() => buttons(container, 'reabrir_sessao_presa')[0].click());
    await act(async () => finish(true));
    expect(reopen.success).toHaveBeenCalledTimes(1);
    expect(reopen.erro).toHaveBeenCalledTimes(1);
    act(() => root.unmount());
  });

  it('Reabrir que falha sem detalhe diz que não recarregou, nunca "dados inválidos"', async () => {
    reopen.recarregar.mockReset();
    reopen.erro.mockClear();
    reopen.recarregar.mockRejectedValue(new Error(''));
    const { container, root } = render({ reopen: { serverId: 'a', name: 'sess' } });
    await act(async () => buttons(container, 'reabrir_sessao_presa')[0].click());
    expect(reopen.erro).toHaveBeenCalledExactlyOnceWith('erro_recarregar: erro_desconhecido');
    act(() => root.unmount());
  });

  it('sem ações (Pi, omp, Kimi): só as linhas; sem entradas, nada', () => {
    const { container, root } = render({ panel: { ...panel, actions: false } });
    expect(container.textContent).toContain('segunda');
    expect(steerButtons(container)).toHaveLength(0);
    expect(buttons(container, 'queue_panel_remove')).toHaveLength(0);
    act(() => root.render(createElement(QueuePanel, { panel: { entries: [], actions: true }, editingId: null, busy: false,
      onEdit: vi.fn(), onCancelEdit: vi.fn(), onSteer: vi.fn(), onRemove: vi.fn(), onMove: vi.fn() })));
    expect(container.textContent).toBe('');
    act(() => root.unmount());
  });
});


describe('QueuePanel: só as mensagens', () => {
  it('o modo de envio saiu do painel (mora nas Configurações > Geral)', () => {
    const { container, root } = render({});
    expect(container.querySelectorAll('[role="radio"]')).toHaveLength(0);
    expect(container.querySelector('[role="radiogroup"]')).toBeNull();
    act(() => root.unmount());
  });

  it('fila vazia: nada aparece', () => {
    const { container, root } = render({ panel: { entries: [], actions: true } });
    expect(container.textContent).toBe('');
    act(() => root.unmount());
  });

  it('mensagem que entra e sai da fila num instante não pisca o painel', () => {
    vi.useFakeTimers();
    try {
      const { container, root, handlers } = render({ panel: { entries: [], actions: true } });
      const props = { editingId: null, busy: false, ...handlers };
      act(() => root.render(createElement(QueuePanel, { panel, ...props })));
      act(() => vi.advanceTimersByTime(300));
      expect(container.textContent).toBe('');
      act(() => root.render(createElement(QueuePanel, { panel: { entries: [], actions: true }, ...props })));
      act(() => root.render(createElement(QueuePanel, { panel, ...props })));
      // O prazo recomeça: o da primeira passagem não pode ter sobrado.
      act(() => vi.advanceTimersByTime(400));
      expect(container.textContent).toBe('');
      act(() => vi.advanceTimersByTime(200));
      expect(container.textContent).toContain('primeira');
      act(() => root.unmount());
    } finally { vi.useRealTimers(); }
  });
});
