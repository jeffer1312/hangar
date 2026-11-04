import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { DEMO_BASE_URL, installDemoFetch, openDemoEventSource } from './demo';

const realFetch = globalThis.fetch;

describe('modo demonstração', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    globalThis.fetch = vi.fn(async () => new Response('rede')) as typeof fetch;
    installDemoFetch();
  });
  afterEach(() => {
    vi.useRealTimers();
    globalThis.fetch = realFetch;
  });

  it('responde lista e histórico sem ir à rede', async () => {
    const list = await (await fetch(`${DEMO_BASE_URL}/api/sessions`)).json() as { name: string }[];
    expect(list.map((s) => s.name)).toContain('landing-page');
    const history = await fetch(`${DEMO_BASE_URL}/api/sessions/landing-page/history?limit=50`);
    expect((await history.json() as unknown[]).length).toBeGreaterThan(0);
    expect(await (await fetch('https://outro.test/x')).text()).toBe('rede');
  });

  it('recusa rota que a demonstração não tem, sem usar 401', async () => {
    expect((await fetch(`${DEMO_BASE_URL}/api/costs`)).status).toBe(404);
  });

  it('publica a lista ao abrir o stream', () => {
    const received: string[] = [];
    const source = openDemoEventSource(`${DEMO_BASE_URL}/api/sessions/events?token=demo`);
    source.addEventListener('sessions', (ev) => received.push(ev.data));
    vi.runOnlyPendingTimers();
    source.close();
    expect(JSON.parse(received[0])[0].name).toBe('landing-page');
  });

  it('responde à mensagem enviada e volta a idle', async () => {
    const states: string[] = [];
    const messages: string[] = [];
    const source = openDemoEventSource(`${DEMO_BASE_URL}/api/sessions/api-tests/events`);
    source.addEventListener('state', (ev) => states.push(JSON.parse(ev.data).state));
    source.addEventListener('message', (ev) => messages.push(JSON.parse(ev.data).kind));
    await fetch(`${DEMO_BASE_URL}/api/sessions/api-tests/input`, { method: 'POST', body: JSON.stringify({ text: 'oi' }) });
    await vi.advanceTimersByTimeAsync(1000);
    source.close();
    expect(messages).toEqual(['user_msg', 'assistant_msg']);
    expect(states.at(-1)).toBe('idle');
  });
});
