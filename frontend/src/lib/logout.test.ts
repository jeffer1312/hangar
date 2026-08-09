// Logout local centralizado (round 2 da 4b): clearCredentials UMA vez com ou sem sync; syncLogout
// best-effort com timeout bounded — pendurado ou rejeitando, o logout local nunca trava nem
// vira unhandled rejection.
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { logoutLocal, type LogoutDeps } from './logout';

function deps(over: Partial<LogoutDeps> = {}): LogoutDeps {
  return {
    temEncKey: true,
    syncLogout: vi.fn(() => Promise.resolve()),
    clearKey: vi.fn(),
    clearCredentials: vi.fn(),
    aoSair: vi.fn(),
    ...over,
  };
}

beforeEach(() => { vi.useFakeTimers(); });
afterEach(() => { vi.useRealTimers(); });

describe('logoutLocal', () => {
  it('sem sync: clearCredentials e aoSair rodam UMA vez, sync não é chamado', async () => {
    const d = deps({ temEncKey: false });
    await logoutLocal(d);
    expect(d.clearCredentials).toHaveBeenCalledTimes(1);
    expect(d.aoSair).toHaveBeenCalledTimes(1);
    expect(d.syncLogout).not.toHaveBeenCalled();
    expect(d.clearKey).not.toHaveBeenCalled();
  });

  it('syncLogout rejeita: logout local segue, sem unhandled', async () => {
    const d = deps({ syncLogout: vi.fn(() => Promise.reject(new Error('hub down'))) });
    await expect(logoutLocal(d)).resolves.toBeUndefined();
    expect(d.clearCredentials).toHaveBeenCalledTimes(1);
    expect(d.clearKey).toHaveBeenCalledTimes(1);
    expect(d.aoSair).toHaveBeenCalledTimes(1);
  });

  it('syncLogout pendurado: timeout bounded libera o logout local', async () => {
    const d = deps({ syncLogout: vi.fn(() => new Promise(() => {})) });
    const promessa = logoutLocal(d);
    vi.advanceTimersByTime(3000);   // hub nunca respondeu
    await promessa;
    expect(d.clearKey).toHaveBeenCalledTimes(1);
    expect(d.aoSair).toHaveBeenCalledTimes(1);
    expect(d.clearCredentials).toHaveBeenCalledTimes(1);
  });

  it('duas origens simultâneas compartilham a MESMA promise: uma sincronização e uma limpeza', async () => {
    const d = deps();
    const p1 = logoutLocal(d);   // origem 1: Sair no drawer
    const p2 = logoutLocal(d);   // origem 2: remover-último, no mesmo instante
    expect(p1).toBe(p2);         // lock in-flight: mesma promise, sem re-executar
    await p1;
    expect(d.clearCredentials).toHaveBeenCalledTimes(1);
    expect(d.syncLogout).toHaveBeenCalledTimes(1);
    expect(d.clearKey).toHaveBeenCalledTimes(1);
    expect(d.aoSair).toHaveBeenCalledTimes(1);
    // lock liberado: um logout NOVO (legítimo, depois de terminar) executa de novo
    await logoutLocal(d);
    expect(d.clearCredentials).toHaveBeenCalledTimes(2);
  });

  it('rejeição tardia do sync com lock: uma limpeza, sem unhandled', async () => {
    const d = deps({ syncLogout: vi.fn(() => new Promise((_, rej) => setTimeout(() => rej(new Error('tarde')), 1000))) });
    const p1 = logoutLocal(d);
    const p2 = logoutLocal(d);
    expect(p1).toBe(p2);
    vi.advanceTimersByTime(1000);   // sync rejeita DEPOIS
    await expect(p1).resolves.toBeUndefined();
    expect(d.clearCredentials).toHaveBeenCalledTimes(1);
    expect(d.clearKey).toHaveBeenCalledTimes(1);
    expect(d.aoSair).toHaveBeenCalledTimes(1);
  });

  it('timeoutMs custom respeita a política', async () => {
    const d = deps({ syncLogout: vi.fn(() => new Promise(() => {})), timeoutMs: 500 });
    const promessa = logoutLocal(d);
    vi.advanceTimersByTime(499);
    expect(d.aoSair).not.toHaveBeenCalled();   // ainda dentro da janela
    vi.advanceTimersByTime(1);
    await promessa;
    expect(d.aoSair).toHaveBeenCalledTimes(1);
  });
});
