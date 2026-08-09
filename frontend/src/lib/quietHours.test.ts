// Máquina de corrida do bloco de horas silenciosas (PushQuiet) — teste determinístico do controller.
// Cenários do round 2 do review: dedup de GET, save+reopen, save pendente, unavailable, timeout.
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { QuietHoursController, type QuietState, type QuietHoursApi, type PushTarget } from './quietHours';
import type { Server } from './auth';

const SERVER: Server = { id: 'srv-a', label: 'A', baseUrl: 'http://a', token: 'x' } as Server;

type PushPayload = { muted: string[]; quiet_hours: { start: string; end: string } | null };

function deferrada<T>() {
  let resolve!: (v: T) => void;
  let reject!: (e: unknown) => void;
  const p = new Promise<T>((res, rej) => { resolve = res; reject = rej; });
  return { p, resolve, reject };
}

function montar() {
  const estado: QuietState = { qhStart: '', qhEnd: '', qhMsg: '', loading: false, saving: false };
  let alvo: PushTarget = { mode: 'global' };
  let open = true;
  // Filas de deferreds: cada chamada de API ganha a PRÓXIMA promessa na ordem — sem corrida de mock.
  const gets: Array<ReturnType<typeof deferrada<PushPayload>>> = [];
  const saves: Array<ReturnType<typeof deferrada<{ ok: boolean }>>> = [];
  const api = {
    getPushSettings: vi.fn<QuietHoursApi['getPushSettings']>(() => { const d = deferrada<PushPayload>(); gets.push(d); return d.p; }),
    getPushSettingsForServer: vi.fn<QuietHoursApi['getPushSettingsForServer']>(),
    setQuietHours: vi.fn<QuietHoursApi['setQuietHours']>(() => { const d = deferrada<{ ok: boolean }>(); saves.push(d); return d.p; }),
    setQuietHoursForServer: vi.fn<QuietHoursApi['setQuietHoursForServer']>(),
  };
  const ctrl = new QuietHoursController({
    estado, getAlvo: () => alvo, getOpen: () => open, podePush: () => true, api,
  });
  return {
    estado, ctrl,
    setAlvo: (t: PushTarget) => { alvo = t; },
    setOpen: (v: boolean) => { open = v; },
    api, gets, saves,
    // Só microtasks: setTimeout é fake (vi.useFakeTimers) e nunca resolveria.
    flush: async () => { await Promise.resolve(); await Promise.resolve(); await Promise.resolve(); },
  };
}

beforeEach(() => { vi.useFakeTimers(); });
afterEach(() => { vi.useRealTimers(); });

describe('load com dedup e sem pintura tardia', () => {
  it('reabrir com load em voo dispara UM GET e a resposta pinta uma vez', async () => {
    const t = montar();
    t.ctrl.sync();
    t.ctrl.sync();                       // "reabrir" durante o load: mesmo alvo, load em voo
    expect(t.api.getPushSettings).toHaveBeenCalledTimes(1);
    expect(t.estado.loading).toBe(true);
    t.gets[0].resolve({ muted: [], quiet_hours: { start: '10:00', end: '11:00' } });
    await t.flush();
    expect(t.estado.qhStart).toBe('10:00');
    expect(t.estado.qhEnd).toBe('11:00');
    expect(t.estado.loading).toBe(false);
  });

  it('resposta tardia após invalidação não pinta nada', async () => {
    const t = montar();
    t.ctrl.sync();                       // load em voo
    t.setAlvo({ mode: 'unavailable' });  // alvo sumiu
    t.ctrl.sync();                       // invalida o load
    t.gets[0].resolve({ muted: [], quiet_hours: { start: '01:00', end: '02:00' } });
    await t.flush();
    expect(t.estado.qhStart).toBe('');   // resposta velha não pinta
    expect(t.estado.qhMsg).toBe('Servidor indisponível');
  });
});

describe('save com serialização e refresh deferido', () => {
  it('save concluído + reabrir recarrega (new/new) com um GET novo', async () => {
    const t = montar();
    t.ctrl.sync();
    t.gets[0].resolve({ muted: [], quiet_hours: { start: '08:00', end: '09:00' } });
    await t.flush();
    t.estado.qhStart = '10:00';          // usuário edita
    t.estado.qhEnd = '11:00';
    void t.ctrl.save();
    t.saves[0].resolve({ ok: true });
    await t.flush();
    expect(t.estado.qhMsg).toBe('silenciado 10:00–11:00');
    // reabrir: draft limpo (salvou), então reload roda — 1 GET novo com valores autoritativos
    t.setOpen(false);
    t.ctrl.sync();
    t.setOpen(true);
    t.ctrl.sync();
    expect(t.api.getPushSettings).toHaveBeenCalledTimes(2);
    t.gets[1].resolve({ muted: [], quiet_hours: { start: '22:00', end: '23:00' } });
    await t.flush();
    expect(t.estado.qhStart).toBe('22:00');
    expect(t.estado.qhEnd).toBe('23:00');
  });

  it('save pendente + reabrir não limpa nem recarrega; refresh roda ao concluir', async () => {
    const t = montar();
    t.ctrl.sync();
    t.gets[0].resolve({ muted: [], quiet_hours: { start: '08:00', end: '09:00' } });
    await t.flush();
    t.estado.qhStart = '10:00';          // draft sujo
    t.estado.qhEnd = '11:00';
    void t.ctrl.save();                  // save pendente
    t.setOpen(false);
    t.ctrl.sync();
    t.setOpen(true);
    t.ctrl.sync();                       // reabrir com save em voo
    expect(t.estado.qhStart).toBe('10:00');      // NÃO limpou o draft
    expect(t.api.getPushSettings).toHaveBeenCalledTimes(1);  // NÃO recarregou
    t.saves[0].resolve({ ok: true });
    await t.flush();
    // refresh deferido: agora sim, 1 GET
    expect(t.api.getPushSettings).toHaveBeenCalledTimes(2);
    t.gets[1].resolve({ muted: [], quiet_hours: { start: '08:00', end: '09:00' } });
    await t.flush();
    expect(t.estado.qhStart).toBe('08:00');
  });

  it('draft sujo do mesmo alvo não é sobrescrito por reload ao reabrir', async () => {
    const t = montar();
    t.ctrl.sync();
    t.gets[0].resolve({ muted: [], quiet_hours: { start: '08:00', end: '09:00' } });
    await t.flush();
    t.estado.qhStart = '10:00';          // editou, não salvou
    t.setOpen(false);
    t.ctrl.sync();
    t.setOpen(true);
    t.ctrl.sync();                       // reabrir: draft sujo => sem GET, sem sobrescrever
    expect(t.api.getPushSettings).toHaveBeenCalledTimes(1);
    expect(t.estado.qhStart).toBe('10:00');
  });
});

describe('transição para unavailable', () => {
  it('libera loading e limpa campos com save em voo', async () => {
    const t = montar();
    t.ctrl.sync();
    t.gets[0].resolve({ muted: [], quiet_hours: { start: '08:00', end: '09:00' } });
    await t.flush();
    void t.ctrl.save();                  // save em voo
    expect(t.estado.saving).toBe(true);
    t.setAlvo({ mode: 'unavailable' });  // servidor sumiu no meio
    t.ctrl.sync();
    expect(t.estado.loading).toBe(false);
    expect(t.estado.saving).toBe(false); // nada fica preso
    expect(t.estado.qhMsg).toBe('Servidor indisponível');
    t.saves[0].resolve({ ok: true });
    await t.flush();
    expect(t.estado.qhStart).toBe('');   // respostas velhas não pintam
    expect(t.estado.qhMsg).toBe('Servidor indisponível');
  });
});

describe('watchdog de timeout', () => {
  it('load preso libera loading sem resposta tardia pintar', async () => {
    const t = montar();
    t.ctrl.sync();
    expect(t.estado.loading).toBe(true);
    vi.advanceTimersByTime(15000);       // API nunca respondeu
    expect(t.estado.loading).toBe(false);
    expect(t.estado.qhMsg).toBe('não foi possível carregar');
    t.gets[0].resolve({ muted: [], quiet_hours: { start: '01:00', end: '02:00' } });
    await t.flush();
    expect(t.estado.qhStart).toBe('');   // tardia não pinta
  });

  it('save preso libera saving', async () => {
    const t = montar();
    t.ctrl.sync();
    t.gets[0].resolve({ muted: [], quiet_hours: { start: '08:00', end: '09:00' } });
    await t.flush();
    void t.ctrl.save();
    expect(t.estado.saving).toBe(true);
    vi.advanceTimersByTime(15000);
    expect(t.estado.saving).toBe(false);
    expect(t.estado.qhMsg).toBe('erro ao salvar');
  });
});
