import { describe, it, expect } from 'vitest';
import {
  defaultShortcuts,
  resolveShortcuts,
  serializeShortcuts,
  sendsDirect,
  type Shortcut,
  type ShortcutSendText,
} from './shortcuts';

describe('resolveShortcuts', () => {
  it('config vazia resolve no conjunto nativo, na ordem de hoje', () => {
    for (const raw of ['', '   ', null, undefined]) {
      expect(resolveShortcuts(raw)).toEqual(defaultShortcuts());
    }
    expect(defaultShortcuts().map((s) => s.id)).toEqual([
      'terminal', 'modo', 'navegador', 'anexos', 'rodar',
    ]);
  });

  it('JSON quebrado ou shape estranho NUNCA lança: cai no conjunto nativo', () => {
    expect(resolveShortcuts('{quebrado')).toEqual(defaultShortcuts());
    expect(resolveShortcuts('{"id":"x"}')).toEqual(defaultShortcuts());
    expect(resolveShortcuts('"texto"')).toEqual(defaultShortcuts());
  });

  it('item individual inválido é descartado sem derrubar a lista', () => {
    const list: Shortcut[] = [
      { id: 'terminal', type: 'internal', action: 'terminal' },
      { id: 'a1', type: 'send_text', label: 'Relatório', text: '/relatorio-pm' },
    ];
    const raw = JSON.stringify([...list, { id: 'x', type: 'foguete' }, { type: 'shell' }]);
    expect(resolveShortcuts(raw)).toEqual(list);
  });

  it('opcional com tipo errado descarta o item; id repetido fica só o primeiro', () => {
    const ok: Shortcut = { id: 'a1', type: 'shell', label: 'Editor', command: 'code .' };
    const raw = JSON.stringify([
      ok,
      { id: 'a2', type: 'shell', label: 'X', command: 'x', icon: 42 },
      { id: 'a3', type: 'send_text', label: 'Y', text: '/y', send_direct: 'nao' },
      { ...ok, label: 'Duplicado' },
    ]);
    expect(resolveShortcuts(raw)).toEqual([ok]);
  });

  it('roundtrip serialize → resolve preserva a lista', () => {
    const list: Shortcut[] = [
      { id: 'a2', type: 'shell', label: 'Editor', command: 'code .', confirm: true },
      { id: 'rodar', type: 'internal', action: 'rodar' },
    ];
    expect(resolveShortcuts(serializeShortcuts(list))).toEqual(list);
  });
});

describe('sendsDirect', () => {
  const base: ShortcutSendText = { id: 'a', type: 'send_text', label: 'X', text: '/x' };
  it('flag ausente = envia direto; só false pré-preenche', () => {
    expect(sendsDirect(base)).toBe(true);
    expect(sendsDirect({ ...base, send_direct: true })).toBe(true);
    expect(sendsDirect({ ...base, send_direct: false })).toBe(false);
  });
});
