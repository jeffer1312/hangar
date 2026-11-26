import { describe, expect, it } from 'vitest';
import type { WhatsAppState } from '@hangar/core';
import {
  asksAccept, autoUpdateFailure, codeOf, failedJob, installAccepted, isWhatsAppState, jobRunning, minutesLeft, ownUpdateEnd, phoneDigits, phrase,
  readErrorShows, shouldPoll, stepIndex, stoppedByBackup, versionLine, viewOf,
} from './whatsapp';

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

  it('parado com dados de um binário mais novo pede a atualização', () => {
    expect(viewOf({ ...base('stopped', null), status_error: 'whatsapp_schema_newer' })).toBe('schema_newer');
    expect(viewOf({ ...base('stopped', null), status_error: null })).toBe('stopped');
    expect(viewOf({ ...base('stopped', null), status_error: 'whatsapp_outro' })).toBe('stopped');
    // De pé, o status_error é de outra coisa (resposta sem assinatura, sem resposta).
    expect(viewOf({ ...base('running', null), status_error: 'whatsapp_untrusted' })).toBe('unreachable');
  });

  it('atualizar que não achou versão mais nova para de oferecer a atualização', () => {
    const after = (kind: string, outcome: unknown): WhatsAppState => ({ ...base('stopped', null), status_error: 'whatsapp_schema_newer',
      job: { kind, step: 'download', done: true, error: null, outcome } });
    expect(viewOf(after('update', 'already_latest'))).toBe('schema_newer_latest');
    expect(viewOf(after('update', 'updated'))).toBe('schema_newer');
    expect(viewOf(after('install', 'already_latest'))).toBe('schema_newer');
    expect(viewOf({ ...after('update', 'already_latest'), status_error: null })).toBe('stopped');
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

  it('depois de uma leitura que falhou relê em qualquer estado', () => {
    expect(shouldPoll(base('running', { state: 'connected' }), false, true)).toBe(true);
    expect(shouldPoll(base('stopped', null), false, true)).toBe(true);
    expect(shouldPoll(base('stopped', null), false)).toBe(false);
  });
});

describe('readErrorShows', () => {
  it('na carga inicial a falha aparece na hora; com dado na tela, só na terceira seguida', () => {
    expect(readErrorShows(false, 1)).toBe(true);
    expect(readErrorShows(true, 1)).toBe(false);
    expect(readErrorShows(true, 2)).toBe(false);
    expect(readErrorShows(true, 3)).toBe(true);
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

describe('phrase', () => {
  const table = { download_failed: () => 'falhou o download' };
  const unknown = ({ code }: { code: string }) => `outro: ${code}`;
  it('tira o prefixo whatsapp_ e cai na genérica com o código cru', () => {
    expect(phrase(table, 'whatsapp_download_failed', unknown)).toBe('falhou o download');
    expect(phrase(table, 'download_failed', unknown)).toBe('falhou o download');
    expect(phrase(table, 'whatsapp_novo_erro', unknown)).toBe('outro: whatsapp_novo_erro');
    expect(phrase(table, 'whatsapp_', unknown)).toBe('outro: whatsapp_');
    // Nome herdado de Object não é frase.
    expect(phrase(table, 'toString', unknown)).toBe('outro: toString');
  });
});

describe('aceite no parado', () => {
  it('sem aceite gravado pede o aceite e só religa marcado; com ele, nada muda', () => {
    const unaccepted = { ...base('stopped', null), accepted_at: null };
    expect(viewOf(unaccepted)).toBe('stopped');
    expect(asksAccept(unaccepted)).toBe(true);
    expect(installAccepted(unaccepted, false)).toBe(false);
    expect(installAccepted(unaccepted, true)).toBe(true);
    const accepted = base('stopped', null);
    expect(asksAccept(accepted)).toBe(false);
    expect(installAccepted(accepted, false)).toBe(true);
  });
});

describe('isWhatsAppState', () => {
  it('recusa serviço desconhecido e resposta sem installed', () => {
    expect(isWhatsAppState(base('running', null))).toBe(true);
    expect(isWhatsAppState({ ...base('running', null), service: 'sleeping' })).toBe(false);
    expect(isWhatsAppState({ service: 'running' })).toBe(false);
    expect(isWhatsAppState(null)).toBe(false);
  });

  it('confere o job e o status que a página desenha, aceitando os nulos', () => {
    const connected = base('running', { state: 'connected', device: '+55 64 ****-1234', device_name: null, totals: { messages: 3 } });
    expect(isWhatsAppState(connected)).toBe(true);
    expect(isWhatsAppState({ ...connected, status: { ...connected.status, device: 123 } })).toBe(false);
    expect(isWhatsAppState({ ...connected, status: { ...connected.status, state: undefined } })).toBe(false);
    expect(isWhatsAppState({ ...connected, status: { ...connected.status, totals: { messages: '3' } } })).toBe(false);
    expect(isWhatsAppState({ ...connected, status: { ...connected.status, error: { code: 7 } } })).toBe(false);
    expect(isWhatsAppState({ ...connected, status: 'connected' })).toBe(false);
    const job = { kind: 'install', step: 'service', done: false, error: null };
    expect(isWhatsAppState({ ...connected, job })).toBe(true);
    expect(isWhatsAppState({ ...connected, job: { ...job, error: { code: 'whatsapp_hash' } } })).toBe(true);
    expect(isWhatsAppState({ ...connected, job: { ...job, done: 'no' } })).toBe(false);
    expect(isWhatsAppState({ ...connected, job: { ...job, step: null } })).toBe(true);
    expect(isWhatsAppState({ ...connected, job: { kind: 'install', done: false, error: null } })).toBe(true);
    expect(isWhatsAppState({ ...connected, job: { ...job, step: 2 } })).toBe(false);
    expect(isWhatsAppState({ ...connected, qr_svg: 5 })).toBe(false);
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

describe('tempo', () => {
  const now = Date.parse('2026-11-22T14:00:00Z');
  it('minutesLeft arredonda para cima e some no passado', () => {
    expect(minutesLeft('2026-11-22T14:02:30Z', now)).toBe(3);
    expect(minutesLeft('2026-11-22T13:59:00Z', now)).toBeNull();
    expect(minutesLeft(undefined, now)).toBeNull();
  });
});

describe('atualização do programa', () => {
  const connected = (extra: Partial<WhatsAppState> = {}): WhatsAppState => ({ ...base('running', { state: 'connected' }), installed_tag: 'v0.3.0', ...extra });
  const newer = (migration: boolean, failed: { tag: string; code: string } | null = null) =>
    ({ latest: 'v0.3.1', migration, checked_at: '2026-11-26T13:48:00Z', failed });

  it('a linha da versão: em dia, sem procura, versão nova, muda o formato e depois de uma falha', () => {
    expect(versionLine(connected({ update: null, update_checked_at: '2026-11-26T13:48:00Z' })))
      .toEqual({ kind: 'current', tag: 'v0.3.0', checkedAt: Date.parse('2026-11-26T13:48:00Z') / 1000 });
    expect(versionLine(connected())).toEqual({ kind: 'current', tag: 'v0.3.0', checkedAt: null });
    expect(versionLine(connected({ update_checked_at: 'ontem' }))).toEqual({ kind: 'current', tag: 'v0.3.0', checkedAt: null });
    expect(versionLine(connected({ update: newer(false) }))).toEqual({ kind: 'available', tag: 'v0.3.0', latest: 'v0.3.1' });
    expect(versionLine(connected({ update: newer(true) }))).toEqual({ kind: 'migration', tag: 'v0.3.0', latest: 'v0.3.1' });
    // A faixa do topo oferece a versão de novo: a linha só diz quando procurou.
    expect(versionLine(connected({ update: newer(false, { tag: 'v0.3.1', code: 'whatsapp_not_started' }) })))
      .toEqual({ kind: 'checked', tag: 'v0.3.0', checkedAt: Date.parse('2026-11-26T13:48:00Z') / 1000 });
  });

  it('a linha só aparece conectado e com a versão instalada conhecida', () => {
    expect(versionLine(connected({ installed_tag: null }))).toBeNull();
    expect(versionLine({ ...connected(), status: { ...connected().status!, state: 'pairing' } })).toBeNull();
    expect(versionLine({ ...connected(), service: 'stopped', status: null })).toBeNull();
    expect(versionLine({ ...connected(), job: { kind: 'auto_update', step: 'service', done: false, error: null } })).toBeNull();
  });

  it('a falha registrada vira a faixa do topo, salvo com job andando ou a faixa do job do dono', () => {
    const failed = { tag: 'v0.3.1', code: 'whatsapp_not_started' };
    expect(autoUpdateFailure(connected({ update: newer(true, failed) })))
      .toEqual({ tag: 'v0.3.1', code: 'whatsapp_not_started', installed: 'v0.3.0', migration: true });
    expect(autoUpdateFailure(connected({ update: newer(false) }))).toBeNull();
    expect(autoUpdateFailure(connected({ update: null }))).toBeNull();
    // Falha antes de mexer no serviço se repete sozinha: não abre faixa.
    expect(autoUpdateFailure(connected({ update: { ...newer(false), last_error: failed } }))).toBeNull();
    expect(autoUpdateFailure(connected({ update: newer(false, failed), job: { kind: 'auto_update', step: 'download', done: false, error: null } }))).toBeNull();
    expect(autoUpdateFailure(connected({ update: newer(false, failed), job: { kind: 'update', step: 'start', done: true, error: 'whatsapp_not_started' } })))
      .toBeNull();
    // A passada automática que terminou não tem faixa de job: a falha aparece por aqui.
    const auto = connected({ update: newer(false, failed), job: { kind: 'auto_update', step: 'start', done: true, error: 'whatsapp_not_started' } });
    expect(failedJob(auto)).toBeNull();
    expect(autoUpdateFailure(auto)).not.toBeNull();
  });

  it('a passada automática em curso mostra a vista de atualizando', () => {
    const s = connected({ job: { kind: 'auto_update', step: 'service', done: false, error: null } });
    expect(viewOf(s)).toBe('working');
    expect(jobRunning(s)).toBe(true);
    expect(jobRunning(connected())).toBe(false);
  });

  it('a confirmação abre sozinha só quando a atualização desta tela parou por mudar o formato', () => {
    const stopped = connected({ update: newer(true), job: { kind: 'update', step: 'download', done: true, error: null, outcome: 'needs_confirm' } });
    expect(ownUpdateEnd(stopped, true)).toBe('confirm');
    expect(ownUpdateEnd(stopped, false)).toBeNull();
    expect(ownUpdateEnd({ ...stopped, job: { ...stopped.job!, kind: 'auto_update' } }, true)).toBe('ended');
    expect(ownUpdateEnd({ ...stopped, job: { ...stopped.job!, outcome: 'updated' } }, true)).toBe('ended');
    expect(ownUpdateEnd({ ...stopped, job: { ...stopped.job!, done: false } }, true)).toBeNull();
    expect(ownUpdateEnd({ ...stopped, job: null }, true)).toBe('ended');
    // Sem as duas versões a folha não tem o que dizer.
    expect(ownUpdateEnd({ ...stopped, update: null }, true)).toBe('ended');
  });

  it('parado porque a cópia antes de converter falhou: o parado comum com outro texto', () => {
    const s = { ...base('stopped', null), status_error: 'whatsapp_backup_failed' };
    expect(viewOf(s)).toBe('stopped');
    expect(stoppedByBackup(s)).toBe(true);
    expect(stoppedByBackup(base('stopped', null))).toBe(false);
    expect(stoppedByBackup({ ...base('running', null), status_error: 'whatsapp_backup_failed' })).toBe(false);
  });

  it('confere installed_tag, update_checked_at e update', () => {
    expect(isWhatsAppState(connected({ update: newer(true, { tag: 'v0.3.1', code: 'x' }), update_checked_at: 'x' }))).toBe(true);
    expect(isWhatsAppState(connected({ update: { ...newer(false), last_error: { tag: 'v0.3.1', code: 'x' } } }))).toBe(true);
    expect(isWhatsAppState({ ...connected(), installed_tag: 3 })).toBe(false);
    expect(isWhatsAppState({ ...connected(), update: { ...newer(false), migration: 'yes' } })).toBe(false);
    expect(isWhatsAppState({ ...connected(), update: { ...newer(false), failed: { tag: 'v0.3.1' } } })).toBe(false);
    expect(isWhatsAppState({ ...connected(), update: { ...newer(false), last_error: 'x' } })).toBe(false);
  });
});
