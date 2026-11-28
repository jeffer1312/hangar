import { expect, it } from 'vitest';
import { settingsValues, type SettingsInput } from './settingsSnapshot';

const base: SettingsInput = {
  theme: 'system', background: 'flat', backgroundEffect: 'none', palette: 'classic', reading: 'auto', textScale: 1, defaultTextScale: 1,
  toolLook: 'claudeDesktop', thinkingTools: 'tudo', taskList: true, tableChart: false, askHighlight: 'amber',
  listGroup: 'server', listOrganize: 'conversations', homeScreen: 'newConversation', delivery: 'queued', handsFree: false, language: 'pt',
};

it('normaliza para os nomes do desktop e da lista do servidor', () => {
  expect(settingsValues(base)).toEqual({
    theme: 'auto', background: 'plain', background_effect: 'none', palette: 'classic', reading: 'auto', text_size: 'normal',
    tool_look: 'claude_desktop', thinking_tools: 'all', task_list: 'on', table_chart: 'off', ask_highlight: 'amber',
    list_group: 'server', list_organize: 'conversations', home_screen: 'new_conversation', delivery: 'queue',
    hands_free: 'off', language: 'pt',
  });
});

it('barra do tablet só no tablet, e o tamanho do texto em faixa', () => {
  const tablet = settingsValues({ ...base, tabletBar: 'left', textScale: 1.2, background: 'aurora', thinkingTools: 'nada' });
  expect(tablet).toMatchObject({ tablet_bar: 'left', text_size: 'large', background: 'aurora', thinking_tools: 'none' });
  expect(settingsValues({ ...base, textScale: 0.9 }).text_size).toBe('small');
  expect(settingsValues(base)).not.toHaveProperty('tablet_bar');
});
