import type { GroupBy, PensamentoTools } from '@hangar/core';
import type { Ferramentas, Fundo, HomeScreen, Idioma, Leitura, Organizar, Tema, DestaquePergunta } from '../stores/aparencia';
import type { SessionsBarPlacement } from '../stores/tabletPrefs';
import type { DeliveryMode } from '../chat/queueDelivery';

export interface SettingsInput {
  theme: Tema;
  background: Fundo;
  backgroundEffect: string;
  palette: string;
  reading: Leitura;
  /** Escala do texto da conversa e a de fábrica. */
  textScale: number;
  defaultTextScale: number;
  toolLook: Ferramentas;
  thinkingTools: PensamentoTools;
  taskList: boolean;
  tableChart: boolean;
  askHighlight: DestaquePergunta;
  listGroup: GroupBy;
  listOrganize: Organizar;
  homeScreen: HomeScreen;
  /** Só no tablet: no celular a barra não existe e o valor guardado seria o de fábrica. */
  tabletBar?: SessionsBarPlacement;
  delivery: DeliveryMode;
  handsFree: boolean;
  language: Idioma;
}

const THINKING = { nada: 'none', busca: 'search', tudo: 'all' } as const;

/** As preferências do aparelho no retrato (`POST /api/telemetry`), nos nomes de `SETTINGS` (hangar-telemetry), iguais aos do desktop. */
export function settingsValues(s: SettingsInput): Record<string, string> {
  const on = (value: boolean) => (value ? 'on' : 'off');
  const values: Record<string, string> = {
    theme: s.theme === 'system' ? 'auto' : s.theme,
    background: s.background === 'flat' ? 'plain' : s.background,
    background_effect: s.backgroundEffect,
    palette: s.palette,
    reading: s.reading,
    text_size: s.textScale < s.defaultTextScale ? 'small' : s.textScale > s.defaultTextScale ? 'large' : 'normal',
    tool_look: s.toolLook === 'claudeDesktop' ? 'claude_desktop' : s.toolLook,
    thinking_tools: THINKING[s.thinkingTools] ?? s.thinkingTools,
    task_list: on(s.taskList),
    table_chart: on(s.tableChart),
    ask_highlight: s.askHighlight,
    list_group: s.listGroup,
    list_organize: s.listOrganize,
    home_screen: s.homeScreen === 'newConversation' ? 'new_conversation' : s.homeScreen,
    delivery: s.delivery === 'queued' ? 'queue' : s.delivery,
    hands_free: on(s.handsFree),
    language: s.language,
  };
  if (s.tabletBar) values.tablet_bar = s.tabletBar;
  return values;
}
