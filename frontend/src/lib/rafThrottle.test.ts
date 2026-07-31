import { describe, it, expect, vi, beforeEach } from 'vitest';
import { rafThrottle } from './rafThrottle';

describe('rafThrottle', () => {
  let rafCb: FrameRequestCallback | null = null;
  let rafCalls = 0;

  beforeEach(() => {
    rafCb = null;
    rafCalls = 0;
    vi.stubGlobal('requestAnimationFrame', (cb: FrameRequestCallback) => {
      rafCalls++;
      rafCb = cb;
      return rafCalls;
    });
    vi.stubGlobal('cancelAnimationFrame', vi.fn());
  });

  it('tres disparos seguidos agendam so 1 quadro, e o callback roda depois do ultimo', () => {
    const callback = vi.fn();
    const { agendar } = rafThrottle(callback);

    agendar();
    agendar();
    agendar();

    expect(rafCalls).toBe(1);
    expect(callback).not.toHaveBeenCalled();

    rafCb?.(0);
    expect(callback).toHaveBeenCalledTimes(1);

    // depois do quadro rodar, o proximo agendar() agenda de novo (nao trava pra sempre)
    agendar();
    expect(rafCalls).toBe(2);
  });

  it('cancelar impede o quadro pendente e permite re-agendar depois', () => {
    const callback = vi.fn();
    const cancelSpy = vi.fn();
    vi.stubGlobal('cancelAnimationFrame', cancelSpy);
    const { agendar, cancelar } = rafThrottle(callback);

    agendar();
    cancelar();
    expect(cancelSpy).toHaveBeenCalledWith(1);

    agendar();
    expect(rafCalls).toBe(2);
  });
});
