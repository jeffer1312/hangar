import { describe, expect, it } from 'vitest';
import type { ChatEvent } from '@hangar/core';
import { desktopSummary, exitCode } from './desktopLook';
import * as m from '../../paraglide/messages';

const call = (id: string, name: string): ChatEvent => ({ id, kind: 'tool_use', tool_use_id: id, tool_name: name, tool_input: {} });
const done = (id: string, error = false): ChatEvent => ({ id: `r-${id}`, kind: 'tool_result', tool_use_id: id, result: 'ok', is_error: error });
const summary = (calls: ChatEvent[], results: ChatEvent[]) => {
  const byId = new Map(results.map((r) => [r.tool_use_id, r]));
  return desktopSummary(calls, (c) => byId.get(c.tool_use_id) ?? null);
};

describe('desktopSummary', () => {
  it('Bash ok, erro, ok, erro -> comandos com falhas', () => {
    const calls = ['a', 'b', 'c', 'd'].map((id) => call(id, 'Bash'));
    expect(summary(calls, [done('a'), done('b', true), done('c'), done('d', true)]))
      .toBe(`${m.ferr_desktop_rodou({ n: 4 })} ${m.ferr_desktop_falhas({ n: 2 })}`);
  });

  it('ToolSearch e WebSearch saem na ordem em que aparecem', () => {
    expect(summary([call('t', 'ToolSearch'), call('w', 'WebSearch')], [done('t'), done('w')]))
      .toBe(`${m.ferr_desktop_ferramentas()}, ${m.ferr_desktop_web_1()}`);
  });

  it('Read, Read, Edit -> leu e editou', () => {
    expect(summary([call('a', 'Read'), call('b', 'Read'), call('c', 'Edit')], [done('a'), done('b'), done('c')]))
      .toBe(`${m.ferr_desktop_leu({ n: 2 })}, ${m.ferr_desktop_editou_1()}`);
  });

  it('chamada sem resultado soma em execução no fim', () => {
    expect(summary([call('a', 'Bash'), call('b', 'Bash')], [done('a')]))
      .toBe(`${m.ferr_desktop_rodou({ n: 2 })}, ${m.ferr_desktop_executando({ n: 1 })}`);
  });

  it('pedido ainda sendo escrito soma como escrevendo, não em execução', () => {
    expect(desktopSummary([call('a', 'Bash')], () => null, true))
      .toBe(`${m.ferr_desktop_rodou_1()}, ${m.ferr_desktop_escrevendo({ n: 1 })}`);
  });

  it('ferramenta de fora usa o nome de exibição', () => {
    expect(summary([call('a', 'mcp__hangar__sessions'), call('b', 'mcp__hangar__sessions')], [done('a'), done('b')]))
      .toBe(m.ferr_desktop_outra({ nome: 'hangar · sessions', n: 2 }));
  });
});

describe('exitCode', () => {
  it('lê o código da primeira linha', () => expect(exitCode('Exit code 1\nerro')).toBe(1));
  it('apara a linha', () => expect(exitCode('  Exit code 127')).toBe(127));
  it('ignora quando não é a primeira linha', () => expect(exitCode('ok\nExit code 1')).toBeNull());
  it('ignora sem número', () => expect(exitCode('Exit code')).toBeNull());
});
