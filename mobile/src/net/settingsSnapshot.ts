import type { GroupBy, PensamentoTools } from '@hangar/core';
import type { Ferramentas, Fundo, HomeScreen, Idioma, Leitura, Organizar, Tema, DestaquePergunta } from '../stores/aparencia';
import type { SessionsBarPlacement } from '../stores/tabletPrefs';
import type { DeliveryMode } from '../chat/queueDelivery';

export interface SettingsInput {
  tema: Tema;
  fundo: Fundo;
  efeito: string;
  paleta: string;
  leitura: Leitura;
  /** Escala do texto da conversa e a de fábrica. */
  texto: number;
  textoPadrao: number;
  ferramentas: Ferramentas;
  pensamentoTools: PensamentoTools;
  tarefas: boolean;
  graficoTabela: boolean;
  destaquePergunta: DestaquePergunta;
  agrupar: GroupBy;
  organizar: Organizar;
  homeScreen: HomeScreen;
  /** Só no tablet: no celular a barra não existe e o valor guardado seria o de fábrica. */
  tabletBar?: SessionsBarPlacement;
  delivery: DeliveryMode;
  handsFree: boolean;
  idioma: Idioma;
}

const THINKING = { nada: 'none', busca: 'search', tudo: 'all' } as const;

/** As preferências do aparelho no `settings.snapshot`, nos nomes de `SETTINGS` (hangar-telemetry), iguais aos do desktop. */
export function settingsValues(s: SettingsInput): Record<string, string> {
  const on = (value: boolean) => (value ? 'on' : 'off');
  const values: Record<string, string> = {
    theme: s.tema === 'system' ? 'auto' : s.tema,
    background: s.fundo === 'flat' ? 'plain' : s.fundo,
    background_effect: s.efeito,
    palette: s.paleta,
    reading: s.leitura,
    text_size: s.texto < s.textoPadrao ? 'small' : s.texto > s.textoPadrao ? 'large' : 'normal',
    tool_look: s.ferramentas === 'claudeDesktop' ? 'claude_desktop' : s.ferramentas,
    thinking_tools: THINKING[s.pensamentoTools] ?? s.pensamentoTools,
    task_list: on(s.tarefas),
    table_chart: on(s.graficoTabela),
    ask_highlight: s.destaquePergunta,
    list_group: s.agrupar,
    list_organize: s.organizar,
    home_screen: s.homeScreen === 'newConversation' ? 'new_conversation' : s.homeScreen,
    delivery: s.delivery === 'queued' ? 'queue' : s.delivery,
    hands_free: on(s.handsFree),
    language: s.idioma,
  };
  if (s.tabletBar) values.tablet_bar = s.tabletBar;
  return values;
}
