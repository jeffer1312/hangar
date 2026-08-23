import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { _createPanePollerForTest } from './usePanePoll';

describe('usePanePoll — porte de TerminalMirror.svelte:75-100', () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => {
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  it('primeiro tick pinta', async () => {
    const getPane = vi.fn(async () => ({ text: 'ola', scrollback: 10 }));
    const poller = _createPanePollerForTest(getPane, 'sess', 200);
    poller.start();
    await vi.advanceTimersByTimeAsync(0);
    await Promise.resolve();
    expect(poller.getState().text).toBe('ola');
    expect(poller.getState().scrollback).toBe(10);
    expect(poller.getState().err).toBe('');
    poller.stop();
  });

  it('texto igual não re-renderiza (contador de sets)', async () => {
    let ret = 'mesmo';
    const getPane = vi.fn(async () => ({ text: ret, scrollback: 0 }));
    const poller = _createPanePollerForTest(getPane, 'sess', 200);
    poller.start();
    await vi.advanceTimersByTimeAsync(0);
    await Promise.resolve();
    expect(poller.getState().setCount).toBe(1);
    // segundo tick com mesmo texto
    await vi.advanceTimersByTimeAsync(450);
    await Promise.resolve();
    expect(getPane).toHaveBeenCalledTimes(2);
    expect(poller.getState().setCount).toBe(1); // não incrementou
    // muda texto
    ret = 'outro';
    await vi.advanceTimersByTimeAsync(450);
    await Promise.resolve();
    expect(poller.getState().setCount).toBe(2);
    poller.stop();
  });

  it('alive=false no unmount cancela o próximo tick', async () => {
    const getPane = vi.fn(async () => ({ text: 'x', scrollback: 0 }));
    const poller = _createPanePollerForTest(getPane, 'sess', 200);
    poller.start();
    await vi.advanceTimersByTimeAsync(0);
    await Promise.resolve();
    expect(getPane).toHaveBeenCalledTimes(1);
    poller.stop();
    await vi.advanceTimersByTimeAsync(1000);
    await Promise.resolve();
    expect(getPane).toHaveBeenCalledTimes(1); // não agendou
  });

  it('erro vira err e o poll continua (finally)', async () => {
    let fail = true;
    const getPane = vi.fn(async () => {
      if (fail) throw new Error('boom');
      return { text: 'ok', scrollback: 0 };
    });
    const poller = _createPanePollerForTest(getPane, 'sess', 200);
    poller.start();
    await vi.advanceTimersByTimeAsync(0);
    await Promise.resolve();
    expect(poller.getState().err).toBe('boom');
    expect(poller.getState().text).toBe('');
    fail = false;
    await vi.advanceTimersByTimeAsync(450);
    await Promise.resolve();
    expect(poller.getState().err).toBe('');
    expect(poller.getState().text).toBe('ok');
    poller.stop();
  });
});
