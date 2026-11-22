import { describe, expect, it } from 'vitest';
import type { WhatsAppState } from '@hangar/core';
import { JOB_ERRORS, codeOf, failedJob, isWhatsAppState, jobError, mediaShares, minutesLeft, phoneDigits, secondsSince, shouldPoll, stepIndex, viewOf } from './whatsapp';

const base = (service: WhatsAppState['service'], status: Partial<NonNullable<WhatsAppState['status']>> | null): WhatsAppState => ({
  installed: true, accepted_at: '2026-11-22T10:00:00Z', service, job: null, qr_svg: null,
  status: status ? { state: 'unpaired', error: null, device: null, pair_code: null, since: null, last_message_at: null, ...status } : null,
});

describe('viewOf', () => {
  it('sem instalar é desligado', () => {
    expect(viewOf({ installed: false, accepted_at: null, service: 'absent', job: null, status: null, qr_svg: null })).toBe('off');
  });

  it('job em curso vence tudo', () => {
    const s: WhatsAppState = { installed: false, accepted_at: 'x', service: 'absent', status: null, qr_svg: null,
      job: { kind: 'install', step: 'service', done: false, error: null } };
    expect(viewOf(s)).toBe('working');
    expect(shouldPoll(s, false)).toBe(true);
  });

  it('serviço instalado: parado, sem resposta e cada estado do C2', () => {
    expect(viewOf(base('stopped', null))).toBe('stopped');
    expect(viewOf(base('absent', null))).toBe('stopped');
    expect(viewOf(base('running', null))).toBe('unreachable');
    expect(viewOf(base('running', { state: 'unpaired' }))).toBe('unpaired');
    expect(viewOf(base('running', { state: 'logged_out' }))).toBe('unpaired');
    expect(viewOf(base('running', { state: 'pairing' }))).toBe('pairing');
    expect(viewOf(base('running', { state: 'outdated' }))).toBe('outdated');
    expect(viewOf(base('running', { state: 'connected' }))).toBe('connected');
    expect(viewOf(base('running', { state: 'novo' }))).toBe('unknown');
  });
});

describe('shouldPoll', () => {
  it('relê no pareamento e, logo depois de pedir, também em unpaired', () => {
    const pairing = base('running', { state: 'pairing' });
    const unpaired = base('running', { state: 'unpaired' });
    expect(shouldPoll(pairing, false)).toBe(true);
    expect(shouldPoll(unpaired, false)).toBe(false);
    expect(shouldPoll(unpaired, true)).toBe(true);
    expect(shouldPoll(base('running', { state: 'connected' }), true)).toBe(false);
  });
});

describe('failedJob', () => {
  it('job terminado com erro traz o tipo e o código, em texto ou objeto', () => {
    const s = { ...base('running', { state: 'connected' }), job: { kind: 'update', step: 'download', done: true, error: { code: 'whatsapp_download_failed' } } };
    expect(failedJob(s)).toEqual({ kind: 'update', code: 'whatsapp_download_failed' });
    expect(viewOf(s)).toBe('connected');
    expect(failedJob({ ...s, job: { ...s.job, error: 'checksum' } })).toEqual({ kind: 'update', code: 'checksum' });
    expect(failedJob({ ...s, job: { ...s.job, error: null } })).toBeNull();
    expect(failedJob({ ...s, job: { ...s.job, done: false } })).toBeNull();
  });
});

describe('jobError', () => {
  it('tira o prefixo whatsapp_ e só reconhece os códigos com frase própria', () => {
    expect(jobError('whatsapp_download_failed')).toBe('download_failed');
    expect(jobError('whatsapp_servico_falhou')).toBe('servico_falhou');
    expect(jobError('accept_write_failed')).toBe('accept_write_failed');
    expect(jobError('whatsapp_novo_erro')).toBeNull();
    expect(jobError('whatsapp_')).toBeNull();
    expect(JOB_ERRORS).toHaveLength(15);
  });
});

describe('isWhatsAppState', () => {
  it('recusa serviço desconhecido e resposta sem installed', () => {
    expect(isWhatsAppState(base('running', null))).toBe(true);
    expect(isWhatsAppState({ ...base('running', null), service: 'sleeping' })).toBe(false);
    expect(isWhatsAppState({ service: 'running' })).toBe(false);
    expect(isWhatsAppState(null)).toBe(false);
  });
});

describe('codeOf', () => {
  it('aceita texto ou {code}, e vazio vira null', () => {
    expect(codeOf('pair_timeout')).toBe('pair_timeout');
    expect(codeOf({ code: 'pair_timeout' })).toBe('pair_timeout');
    expect(codeOf({ code: '' })).toBeNull();
    expect(codeOf(3)).toBeNull();
    expect(codeOf(null)).toBeNull();
  });
});

describe('stepIndex', () => {
  it('etapa desconhecida conta como a primeira', () => {
    expect(stepIndex('download')).toBe(0);
    expect(stepIndex('service')).toBe(1);
    expect(stepIndex('start')).toBe(2);
    expect(stepIndex('whatever')).toBe(0);
    expect(stepIndex(undefined)).toBe(0);
  });
});

describe('phoneDigits', () => {
  it('fica só com os algarismos, de 10 a 15', () => {
    expect(phoneDigits('+55 (64) 99999-1234')).toBe('5564999991234');
    expect(phoneDigits('64 9999')).toBeNull();
    expect(phoneDigits('5564abc99991234')).toBeNull();
    expect(phoneDigits('1234567890123456')).toBeNull();
    expect(phoneDigits('5564.9999.1234')).toBe('556499991234');
  });
});

describe('mediaShares', () => {
  it('divide a barra pelo total e zera sem mídia', () => {
    expect(mediaShares({ media_saved: 4195, media_phone_only: 3501, media_lost: 10 }).saved).toBeCloseTo(4195 / 7706);
    expect(mediaShares({ media_saved: 1, media_phone_only: 1, media_lost: 2 })).toEqual({ saved: 0.25, phoneOnly: 0.25, lost: 0.5 });
    expect(mediaShares(undefined)).toEqual({ saved: 0, phoneOnly: 0, lost: 0 });
  });
});

describe('tempo', () => {
  const now = Date.parse('2026-11-22T14:00:00Z');
  it('secondsSince lê o ISO UTC e não fica negativo', () => {
    expect(secondsSince('2026-11-22T13:58:00Z', now)).toBe(120);
    expect(secondsSince('2026-11-22T14:01:00Z', now)).toBe(0);
    expect(secondsSince('ontem', now)).toBeNull();
    expect(secondsSince(null, now)).toBeNull();
  });

  it('minutesLeft arredonda para cima e some no passado', () => {
    expect(minutesLeft('2026-11-22T14:02:30Z', now)).toBe(3);
    expect(minutesLeft('2026-11-22T13:59:00Z', now)).toBeNull();
    expect(minutesLeft(undefined, now)).toBeNull();
  });
});
