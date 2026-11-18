import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { PANEL_TAB_REQUEST_TTL_MS, usePanelTabRequest } from './panelTab';

beforeEach(() => { vi.useFakeTimers(); usePanelTabRequest.setState({ request: null }); });
afterEach(() => { vi.useRealTimers(); });

describe('usePanelTabRequest', () => {
  it('pedido que nenhum painel consumiu expira e não abre a sessão em Revisão depois', () => {
    usePanelTabRequest.getState().open('srv', 'pm', 'review');
    expect(usePanelTabRequest.getState().request).toEqual({ key: 'srv::pm', tab: 'review' });
    vi.advanceTimersByTime(PANEL_TAB_REQUEST_TTL_MS);
    expect(usePanelTabRequest.getState().request).toBeNull();
  });

  it('pedido novo não é apagado pelo prazo do anterior', () => {
    usePanelTabRequest.getState().open('srv', 'pm', 'review');
    vi.advanceTimersByTime(PANEL_TAB_REQUEST_TTL_MS - 100);
    usePanelTabRequest.getState().open('srv', 'outra', 'review');
    vi.advanceTimersByTime(200);
    expect(usePanelTabRequest.getState().request).toEqual({ key: 'srv::outra', tab: 'review' });
    vi.advanceTimersByTime(PANEL_TAB_REQUEST_TTL_MS);
    expect(usePanelTabRequest.getState().request).toBeNull();
  });

  it('o painel da sessão consome o pedido; outra chave não o apaga', () => {
    usePanelTabRequest.getState().open('srv', 'pm', 'review');
    usePanelTabRequest.getState().clear('srv::outra');
    expect(usePanelTabRequest.getState().request?.key).toBe('srv::pm');
    usePanelTabRequest.getState().clear('srv::pm');
    expect(usePanelTabRequest.getState().request).toBeNull();
  });
});
