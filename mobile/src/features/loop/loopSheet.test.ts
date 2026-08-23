import { describe, expect, it } from 'vitest';
import type { LoopState } from '@hangar/core';
import * as m from '../../paraglide/messages';
import { cleanErr, isFinal, isForm, isPolling } from './loopSheet';

const makeLoop = (status: LoopState['status']): LoopState => ({
  goal: 'objetivo',
  check_cmd: null,
  max_iters: 2,
  require_branch: true,
  status,
  iter: 1,
  history: [],
  started_ts: 0,
  ended_ts: null,
  ended_reason: null,
});

describe('loopSheet', () => {
  it('reconhece todos os estados finais e rejeita os ativos', () => {
    expect(['done', 'stopped', 'exhausted', 'failed'].every((status) => isFinal(makeLoop(status as LoopState['status'])))).toBe(true);
    expect(isFinal(makeLoop('running'))).toBe(false);
  });

  it('mostra o formulário sem loop ou quando um estado final foi reiniciado', () => {
    expect(isForm(null, false)).toBe(true);
    expect(isForm(makeLoop('done'), false)).toBe(false);
    expect(isForm(makeLoop('done'), true)).toBe(true);
    expect(isForm(makeLoop('running'), true)).toBe(false);
  });

  it('considera polling somente os estados não finais', () => {
    expect(isPolling(makeLoop('running'))).toBe(true);
    expect(isPolling(makeLoop('done_claimed'))).toBe(true);
    expect(isPolling(makeLoop('exhausted'))).toBe(false);
    expect(isPolling(null)).toBe(false);
  });

  it('limpa prefixo HTTP e traduz falha de rede', () => {
    expect(cleanErr(new Error('409: loop já ativo'))).toBe('loop já ativo');
    expect(cleanErr(new Error('Failed to fetch'))).toBe(m.loop_servidor_nao_respondeu());
    expect(cleanErr(new Error('12: erro do servidor'))).toBe('erro do servidor');
  });
});
