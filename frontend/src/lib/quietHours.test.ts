// Máquina de corrida do bloco de horas silenciosas (PushQuiet) — teste determinístico do controller.
// Cenários do round 2 do review: dedup de GET, save+reopen, save pendente, unavailable, timeout.
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { QuietHoursController, type QuietState, type QuietHoursApi, type PushTarget } from './quietHours';
import type { Server } from './auth';
import { overwriteGetLocale } from '../paraglide/runtime';

beforeEach(() => overwriteGetLocale(() => 'pt'));


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
    getPushSettingsForServer: vi.fn<QuietHoursApi['getPushSettingsForServer']>(() => { const d = deferrada<PushPayload>(); gets.push(d); return d.p; }),
    setQuietHours: vi.fn<QuietHoursApi['setQuietHours']>(() => { const d = deferrada<{ ok: boolean }>(); saves.push(d); return d.p; }),
    setQuietHoursForServer: vi.fn<QuietHoursApi['setQuietHoursForServer']>(() => { const d = deferrada<{ ok: boolean }>(); saves.push(d); return d.p; }),
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

describe('ownership de save (round 3)', () => {
  it('save A invalidado por troca de alvo não derruba o saving do save B; sem double save', async () => {
    const t = montar();
    t.ctrl.sync();
    t.gets[0].resolve({ muted: [], quiet_hours: { start: '08:00', end: '09:00' } });
    await t.flush();
    t.estado.qhStart = '10:00';
    t.estado.qhEnd = '11:00';
    void t.ctrl.save();                    // save A (global) em voo
    t.setAlvo({ mode: 'server', server: SERVER });   // alvo trocou
    t.ctrl.sync();                         // invalida A, limpa campos e inicia load do alvo novo
    expect(t.estado.saving).toBe(false);   // invalidação derrubou a flag do A
    t.gets[1].resolve({ muted: [], quiet_hours: { start: '00:00', end: '01:00' } });
    await t.flush();                       // load do alvo novo conclui (loading=false)
    t.estado.qhStart = '12:00';
    t.estado.qhEnd = '13:00';
    void t.ctrl.save();                    // save B (server) em voo
    expect(t.estado.saving).toBe(true);
    t.saves[0].resolve({ ok: true });      // A resolve DEPOIS de B começar
    await t.flush();
    expect(t.estado.saving).toBe(true);    // B continua dono: flag intacta
    expect(t.api.setQuietHoursForServer).toHaveBeenCalledTimes(1);
    expect(t.api.setQuietHours).toHaveBeenCalledTimes(1);
    t.saves[1].resolve({ ok: true });      // B conclui
    await t.flush();
    expect(t.estado.saving).toBe(false);
  });
});

describe('refresh deferido só no sucesso (round 3)', () => {
  it('save falha após reopen: sem refresh, flag limpa, draft e erro preservados', async () => {
    const t = montar();
    t.ctrl.sync();
    t.gets[0].resolve({ muted: [], quiet_hours: { start: '08:00', end: '09:00' } });
    await t.flush();
    t.estado.qhStart = '10:00';
    t.estado.qhEnd = '11:00';
    const d2 = deferrada<{ ok: boolean }>();
    t.api.setQuietHours.mockReturnValueOnce(d2.p);
    void t.ctrl.save();                    // save pendente
    t.setOpen(false);
    t.ctrl.sync();
    t.setOpen(true);
    t.ctrl.sync();                         // reopen: refreshDepois = true
    d2.reject(new Error('HTTP 500'));
    await t.flush();
    expect(t.api.getPushSettings).toHaveBeenCalledTimes(1);   // refresh NÃO rodou
    expect(t.estado.qhStart).toBe('10:00');                   // draft preservado
    expect(t.estado.qhMsg).toBe('HTTP 500');                  // erro preservado
    // próximo reopen: refreshDepois limpo, mas draft sujo ainda bloqueia reload
    t.setOpen(false);
    t.ctrl.sync();
    t.setOpen(true);
    t.ctrl.sync();
    expect(t.api.getPushSettings).toHaveBeenCalledTimes(1);
  });

  it('timeout do save após reopen: refresh cancelado, nada fica preso', async () => {
    const t = montar();
    t.ctrl.sync();
    t.gets[0].resolve({ muted: [], quiet_hours: { start: '08:00', end: '09:00' } });
    await t.flush();
    t.estado.qhStart = '10:00';
    t.estado.qhEnd = '11:00';
    const d2 = deferrada<{ ok: boolean }>();
    t.api.setQuietHours.mockReturnValueOnce(d2.p);
    void t.ctrl.save();
    t.setOpen(false);
    t.ctrl.sync();
    t.setOpen(true);
    t.ctrl.sync();                         // refreshDepois = true
    vi.advanceTimersByTime(15000);         // save estoura
    expect(t.estado.saving).toBe(false);
    expect(t.estado.qhMsg).toBe('erro ao salvar');
    d2.resolve({ ok: true });              // resposta tardia
    await t.flush();
    expect(t.api.getPushSettings).toHaveBeenCalledTimes(1);   // sem refresh
    expect(t.estado.qhStart).toBe('10:00');
  });

  it('refresh nunca roda com o menu fechado', async () => {
    const t = montar();
    t.ctrl.sync();
    t.gets[0].resolve({ muted: [], quiet_hours: { start: '08:00', end: '09:00' } });
    await t.flush();
    t.estado.qhStart = '10:00';
    t.estado.qhEnd = '11:00';
    const d2 = deferrada<{ ok: boolean }>();
    t.api.setQuietHours.mockReturnValueOnce(d2.p);
    void t.ctrl.save();
    t.setOpen(false);
    t.ctrl.sync();
    t.setOpen(true);
    t.ctrl.sync();                         // refreshDepois = true
    t.setOpen(false);                      // fecha ANTES do save concluir
    d2.resolve({ ok: true });
    await t.flush();
    expect(t.api.getPushSettings).toHaveBeenCalledTimes(1);   // fechado: sem GET
  });
});

describe('dispose (round 3)', () => {
  it('load pendente + dispose: resposta tardia não pinta e nada inicia depois', async () => {
    const t = montar();
    t.ctrl.sync();
    expect(t.estado.loading).toBe(true);
    t.ctrl.dispose();
    t.gets[0].resolve({ muted: [], quiet_hours: { start: '01:00', end: '02:00' } });
    await t.flush();
    expect(t.estado.qhStart).toBe('');
    t.ctrl.sync();                          // sync pós-dispose é no-op
    expect(t.api.getPushSettings).toHaveBeenCalledTimes(1);
  });

  it('save pendente + dispose: resposta tardia não pinta nada', async () => {
    const t = montar();
    t.ctrl.sync();
    t.gets[0].resolve({ muted: [], quiet_hours: { start: '08:00', end: '09:00' } });
    await t.flush();
    t.estado.qhStart = '10:00';
    t.estado.qhEnd = '11:00';
    const d2 = deferrada<{ ok: boolean }>();
    t.api.setQuietHours.mockReturnValueOnce(d2.p);
    void t.ctrl.save();
    t.ctrl.dispose();
    d2.resolve({ ok: true });
    await t.flush();
    expect(t.estado.qhMsg).toBe('');        // nada pintou
    expect(t.api.getPushSettings).toHaveBeenCalledTimes(1);
  });

  it('múltiplos controllers são isolados', async () => {
    const t = montar();
    const gets2: Array<ReturnType<typeof deferrada<PushPayload>>> = [];
    const estado2: QuietState = { qhStart: '', qhEnd: '', qhMsg: '', loading: false, saving: false };
    const api2 = {
      getPushSettings: vi.fn<QuietHoursApi['getPushSettings']>(() => { const d = deferrada<PushPayload>(); gets2.push(d); return d.p; }),
      getPushSettingsForServer: vi.fn<QuietHoursApi['getPushSettingsForServer']>(),
      setQuietHours: vi.fn<QuietHoursApi['setQuietHours']>(),
      setQuietHoursForServer: vi.fn<QuietHoursApi['setQuietHoursForServer']>(),
    };
    const ctrl2 = new QuietHoursController({
      estado: estado2, getAlvo: () => ({ mode: 'global' } as PushTarget), getOpen: () => true, podePush: () => true, api: api2,
    });
    t.ctrl.sync();
    ctrl2.sync();
    ctrl2.dispose();                        // dispose de um não afeta o outro
    t.gets[0].resolve({ muted: [], quiet_hours: { start: '10:00', end: '11:00' } });
    await t.flush();
    expect(t.estado.qhStart).toBe('10:00'); // o vivo continua funcionando
    expect(t.api.getPushSettings).toHaveBeenCalledTimes(1);
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
