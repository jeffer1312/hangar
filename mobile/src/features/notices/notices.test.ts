import { describe, expect, test } from 'vitest';
import type { ChatEvent } from '@hangar/core';
import { isApiError, noticeCount, noticeDetail, noticeKey, noticeTitle, parseNoticeIgnores } from './notices';

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

  test('lista ignorada: aceita a rota e o evento, recusa o torto', () => {
    expect(parseNoticeIgnores('{"keys":["hook:Stop"]}')).toEqual(['hook:Stop']);
    expect(parseNoticeIgnores({ keys: [] })).toEqual([]);
    expect(parseNoticeIgnores('{"keys":[1]}')).toBeNull();
    expect(parseNoticeIgnores('nada')).toBeNull();
  });
});
