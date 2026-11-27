import { describe, expect, test } from 'vitest';
import type { ChatEvent } from '@hangar/core';
import { autoContinueCancellable, autoContinueText, fixStates, fixTracker, isApiError, isAutoContinue, isFixRunningNote, noticeAction, sameFixStates, noticeCount, noticeDetail, noticeKey, noticeKeyLabel, noticeTitle, parseNoticeIgnores } from './notices';

const ev = (text: string, notice_meta: ChatEvent['notice_meta'], extra: Partial<ChatEvent> = {}): ChatEvent =>
  ({ kind: 'assistant_msg', id: 'local-1', text, notice_meta, ...extra }) as ChatEvent;

describe('aviso da CLI do Claude', () => {
  test('nota da CLI: título é a primeira linha sem a engrenagem, o resto abre ao tocar', () => {
    const note = ev('⚙️ Servidor MCP que precisa de login: `slack`\nO plugin `x` não carregou', { key: 'note:startup:ab12cd34' });
    expect(noticeTitle(note)).toBe('Servidor MCP que precisa de login: `slack`');
    expect(noticeDetail(note)).toContain('O plugin `x` não carregou');
    expect(noticeKey(note)).toBe('note:startup:ab12cd34');
  });

  test('hook: título pelo nome, total só acima de uma falha', () => {
    const hook = ev('hook_failed', { key: 'hook:Stop', subject: 'Stop', count: 12 }, { kind: 'notice', hook_error: 'Stop: sem pasta' });
    expect(noticeTitle(hook)).toContain('Stop');
    expect(noticeCount(hook)).toContain('12');
    expect(noticeDetail(hook)).toBe('Stop: sem pasta');
    expect(noticeCount(ev('hook_failed', { key: 'hook:Stop', subject: 'Stop', count: 1 }))).toBeNull();
  });

  test('erro da API é bloqueio: sem chave de ignorar', () => {
    const error = ev('Failed to refresh OAuth token', { level: 'error' });
    expect(isApiError(error)).toBe(true);
    expect(noticeKey(error)).toBeNull();
  });

  test('ação do aviso vem do servidor; reinstalar sem plugin não tem botão', () => {
    expect(noticeAction(ev('x', { action: 'reinstall_plugin', plugin: 'pl@mk' }))).toEqual({ kind: 'reinstall', plugin: 'pl@mk' });
    expect(noticeAction(ev('x', { action: 'fix_hook' }))).toEqual({ kind: 'fix' });
    expect(noticeAction(ev('x', { action: 'reinstall_plugin' }))).toBeNull();
    expect(noticeAction(ev('x', {}))).toBeNull();
    expect(noticeKeyLabel('note:plugin:pl@mk')).toContain('pl@mk');
  });

  test('conserto: a tentativa mais nova decide; o andamento não tem linha própria', () => {
    const fix = (id: string, attempt: number, state: string) => ev('x', { fix_of: 'hookfail:a', attempt, state, child: 'conserto-s' }, { id });
    const running = fix('f1', 1, 'running');
    expect(isFixRunningNote(running)).toBe(true);
    expect(fixStates([running]).get('hookfail:a')).toEqual({ state: 'running', child: 'conserto-s' });
    const done = fix('d1', 1, 'done');
    expect(isFixRunningNote(done)).toBe(false);
    expect(fixStates([running, done]).get('hookfail:a')).toEqual({ state: 'done' });
    expect(fixStates([running, done, fix('f2', 2, 'running')]).get('hookfail:a')?.state).toBe('running');
    expect(fixStates([fix('x', 1, 'failed')]).get('hookfail:a')).toEqual({ state: 'failed' });
    expect(fixStates([done]).get('hookfail:b')).toBeUndefined();
    expect(sameFixStates(fixStates([running]), fixStates([running]))).toBe(true);
    expect(sameFixStates(fixStates([running]), fixStates([running, done]))).toBe(false);
  });

  test('acompanhamento lê só o que chegou e guarda o mesmo mapa sem conserto novo', () => {
    const fix = (id: string, attempt: number, state: string) => ev('x', { fix_of: 'hookfail:a', attempt, state, child: 'c' }, { id });
    const track = fixTracker();
    const first = [fix('f1', 1, 'running')];
    const a = track(first);
    const b = track([...first, ev('oi', null, { id: 'm1' })]);
    expect(b).toBe(a);
    expect(track([...first, ev('oi', null, { id: 'm1' }), fix('d1', 1, 'done')]).get('hookfail:a')).toEqual({ state: 'done' });
    expect(track([ev('outra', null, { id: 'z' })]).size).toBe(0);
  });

  test('troca no meio da lista (revisão nova) relê tudo, mesmo com o último evento igual', () => {
    const fix = (id: string, state: string) => ev('x', { fix_of: 'hookfail:a', attempt: 1, state, child: 'c' }, { id });
    const tail = ev('oi', null, { id: 'm1' });
    const track = fixTracker();
    expect(track([fix('f1', 'running'), tail], 1).get('hookfail:a')?.state).toBe('running');
    expect(track([fix('f1', 'failed'), tail], 2).get('hookfail:a')).toEqual({ state: 'failed' });
  });

  // Horário local: o texto mostra HH:MM do aparelho, em qualquer fuso.
  const at = (hour: number, minute: number) => new Date(2026, 10, 27, hour, minute).getTime() / 1000;
  const autoNote = (state: string, extra: Record<string, unknown> = {}) =>
    ev('texto do servidor', { kind: 'auto_continue', state, stopped_at: at(10, 5), count: 1, ...extra } as ChatEvent['notice_meta']);

  test('continuar sozinho: a nota lê os horários do meta, trocada a cada passo', () => {
    expect(isAutoContinue(autoNote('waiting'))).toBe(true);
    expect(autoContinueText(autoNote('waiting'))).toBe('A conexão caiu às 10:05; continuo quando a rede voltar.');
    expect(autoContinueText(autoNote('continued', { continued_at: at(10, 20), count: 2 }))).toBe('A conexão caiu às 10:05; continuei às 10:20.');
    expect(autoContinueText(autoNote('failed', { code: 'erro_envio_falhou' }))).toBe('A conexão caiu às 10:05; não consegui continuar (erro_envio_falhou).');
    expect(autoContinueText(autoNote('cancelled'))).toBe('A conexão caiu às 10:05; não continuei (cancelado).');
    expect(autoContinueText(autoNote('skipped', { code: 'user_after_stop' }))).toBe('A conexão caiu às 10:05; não continuei.');
    // Reboot: `stopped_at` é a hora do boot, o texto diz só quando continuou.
    expect(autoContinueText(autoNote('continued', { cause: 'reboot', continued_at: at(10, 20) }))).toBe('O computador reiniciou no meio do turno; continuei às 10:20.');
    expect(autoContinueText(autoNote('failed', { cause: 'reboot', code: 'erro_envio_falhou' }))).toBe('O computador reiniciou no meio do turno; não consegui continuar (erro_envio_falhou).');
    expect(autoContinueText(autoNote('de-uma-versao-futura'))).toBe('texto do servidor');
    expect(isAutoContinue(ev('⚙️ x', { state: 'waiting' }))).toBe(false);
  });

  test('continuar sozinho: Cancelar só na nota que espera a rede', () => {
    expect(autoContinueCancellable(autoNote('waiting'))).toBe(true);
    for (const state of ['continued', 'failed', 'cancelled', 'skipped']) expect(autoContinueCancellable(autoNote(state))).toBe(false);
    expect(autoContinueCancellable(autoNote('waiting', { cause: 'reboot' }))).toBe(false);
    expect(autoContinueCancellable(ev('x', { fix_of: 'hookfail:a', attempt: 1, state: 'waiting' }))).toBe(false);
  });

  test('lista ignorada: aceita a rota e o evento, recusa o torto', () => {
    expect(parseNoticeIgnores('{"keys":["hook:Stop"]}')).toEqual(['hook:Stop']);
    expect(parseNoticeIgnores({ keys: [] })).toEqual([]);
    expect(parseNoticeIgnores('{"keys":[1]}')).toBeNull();
    expect(parseNoticeIgnores('nada')).toBeNull();
  });
});
