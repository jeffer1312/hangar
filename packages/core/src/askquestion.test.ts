import { describe, test, expect } from 'vitest';
import type { ChatEvent } from './types';
import { pendingAskFromEvents, askPayloadFromToolUse } from './askquestion';

function ev(partial: Partial<ChatEvent> & { id: string }): ChatEvent {
  return {
    kind: 'tool_use',
    tool_name: 'question',
    tool_input: {},
    tool_use_id: 'tu1',
    ts: 1_700_000_000,
    ...partial,
  } as ChatEvent;
}

describe('askquestion — pendingAskFromEvents e askPayloadFromToolUse', () => {
  test('Pi com question+options → 1 pergunta', () => {
    const q = ev({
      id: 'a:1',
      kind: 'tool_use',
      tool_name: 'question',
      tool_use_id: 'tu1',
      tool_input: { question: 'Q?', header: 'H', options: [{ label: 'A', description: 'desc' }], multiSelect: false },
    });
    const payload = askPayloadFromToolUse(q, 'pi');
    expect(payload?.questions).toHaveLength(1);
    expect(payload?.questions[0].question).toBe('Q?');
    expect(payload?.questions[0].options[0].label).toBe('A');
  });

  test('Kimi com questions[{multi_select:true}] → multiSelect: true', () => {
    const q = ev({
      id: 'a:2',
      kind: 'tool_use',
      tool_name: 'AskUserQuestion',
      tool_use_id: 'tu2',
      tool_input: {
        questions: [
          { header: 'H', question: 'Q2?', multi_select: true, options: [{ label: 'O1', description: '' }, { label: 'O2', description: '' }] },
        ],
      },
    });
    const payload = askPayloadFromToolUse(q, 'kimi');
    expect(payload?.questions[0].multiSelect).toBe(true);
    expect(payload?.questions[0].options).toHaveLength(2);
  });

  test('pendente some quando chega tool_result com o mesmo id', () => {
    const toolUse = ev({ id: 'a:1', kind: 'tool_use', tool_name: 'question', tool_use_id: 'tu1', tool_input: { question: 'Q', options: [{ label: 'a', description: '' }] } });
    const toolResult = { kind: 'tool_result', id: 'a:2', tool_use_id: 'tu1', ts: 1 } as ChatEvent;
    const pending = pendingAskFromEvents([toolUse, toolResult], 'pi');
    expect(pending).toBeNull();
    const pending2 = pendingAskFromEvents([toolUse], 'pi');
    expect(pending2?.id).toBe('a:1');
  });

  test('shape sem options → null', () => {
    const q = ev({
      id: 'a:3',
      kind: 'tool_use',
      tool_name: 'question',
      tool_use_id: 'tu3',
      tool_input: { question: 'Q?' },
    });
    expect(askPayloadFromToolUse(q, 'pi')).toBeNull();
    const qKimi = ev({
      id: 'a:4',
      kind: 'tool_use',
      tool_name: 'AskUserQuestion',
      tool_use_id: 'tu4',
      tool_input: { questions: [{ header: '', question: 'Q', multi_select: false, options: [] }] },
    });
    expect(askPayloadFromToolUse(qKimi, 'kimi')).toBeNull();
  });
});
