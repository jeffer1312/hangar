import { afterEach, beforeEach, describe, expect, test, vi } from 'vitest';
import { boxIn, FLIGHT_MS, flies, flying, land, lands, sameText, setFlightHost, takeoff, useSendFlight } from './sendFlight';

const at = (x: number, y: number, width: number, height: number) => ({
  measureInWindow: (done: (x: number, y: number, width: number, height: number) => void) => done(x, y, width, height),
});

describe('voo da mensagem enviada', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    useSendFlight.setState({ flight: null });
    // Área do chat: 400 de largura, de y=100 a y=700.
    setFlightHost('s::a', at(0, 100, 400, 600));
  });
  afterEach(() => {
    setFlightHost('s::a', null);
    vi.useRealTimers();
  });

  test('só texto simples e curto viaja', () => {
    expect(flies('oi')).toBe(true);
    expect(flies('  ')).toBe(false);
    expect(flies('a'.repeat(401))).toBe(false);
    expect(flies('1\n2\n3\n4\n5\n6\n7')).toBe(false);
  });

  test('o mesmo texto não conta espaço nem quebra de linha', () => {
    expect(sameText('a  b\nc', ' a b c ')).toBe(true);
    expect(sameText('a b', 'a c')).toBe(false);
  });

  test('o lugar é contado da borda de baixo da área', () => {
    expect(boxIn({ x: 20, y: 640, width: 300, height: 40 }, { x: 0, y: 100, width: 400, height: 600 }))
      .toEqual({ left: 20, bottom: 20, width: 300, height: 40 });
  });

  test('destino fora da área não recebe viagem', () => {
    expect(lands({ left: 0, bottom: 10, width: 100, height: 40 }, 600)).toBe(true);
    expect(lands({ left: 0, bottom: -30, width: 100, height: 40 }, 600)).toBe(false);
    expect(lands({ left: 0, bottom: 580, width: 100, height: 40 }, 600)).toBe(false);
    expect(lands({ left: 0, bottom: 10, width: 0, height: 40 }, 600)).toBe(false);
  });

  test('decola do campo, pousa no destino medido e some depois do voo', () => {
    takeoff('s::a', 'oi', at(20, 640, 300, 40));
    const waiting = useSendFlight.getState().flight;
    expect(waiting).toMatchObject({ route: 's::a', text: 'oi', to: null, from: { left: 20, bottom: 20, width: 300, height: 40 } });
    expect(flying(waiting, 's::a', ' oi ')).toBe(true);
    expect(flying(waiting, 's::b', 'oi')).toBe(false);

    land('s::a', 'oi', 'chat', at(300, 560, 80, 36), true);
    expect(useSendFlight.getState().flight).toMatchObject({ target: 'chat', dim: true, to: { left: 300, bottom: 104, width: 80, height: 36 } });
    // O destino já medido não muda no meio da viagem.
    land('s::a', 'oi', 'queue', at(0, 600, 10, 10));
    expect(useSendFlight.getState().flight?.target).toBe('chat');

    vi.advanceTimersByTime(FLIGHT_MS + 100);
    expect(useSendFlight.getState().flight).toBeNull();
  });

  test('sem destino a cópia some sozinha, e destino fora da tela cancela a viagem', () => {
    takeoff('s::a', 'oi', at(20, 640, 300, 40));
    vi.advanceTimersByTime(700);
    expect(useSendFlight.getState().flight).toBeNull();

    takeoff('s::a', 'oi', at(20, 640, 300, 40));
    land('s::a', 'oi', 'chat', at(300, 900, 80, 36));
    expect(useSendFlight.getState().flight).toBeNull();
  });

  test('destino de outro texto ou sem área registrada não mexe em nada', () => {
    takeoff('s::a', 'oi', at(20, 640, 300, 40));
    land('s::a', 'outra', 'chat', at(300, 560, 80, 36));
    expect(useSendFlight.getState().flight?.to).toBeNull();
    useSendFlight.setState({ flight: null });
    takeoff('s::b', 'oi', at(20, 640, 300, 40));
    expect(useSendFlight.getState().flight).toBeNull();
  });
});
