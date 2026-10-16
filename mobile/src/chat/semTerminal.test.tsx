// @vitest-environment happy-dom
import { act, createElement, useState, type ReactNode } from 'react';
import { createRoot } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { configureApi, estilosDitado, fetchSessionsForServer, implementCodexPlan, interrupt, loadLiveVoiceSettings, recarregarSessao, setPermissionMode, _limparEsfriamentoParaTestes, _resetRotasForTests, type FirstConversationAttempt, type MotivoFim, type QueuePanel, type SessionInfo } from '@hangar/core';

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const appLifecycle = vi.hoisted(() => ({
  currentState: 'active' as import('react-native').AppStateStatus,
  listeners: new Set<(state: import('react-native').AppStateStatus) => void>(),
}));
const layouts = vi.hoisted(() => new Map<string, (event: import('react-native').LayoutChangeEvent) => void>());
vi.mock('react-native', async (original) => {
  const native = await original<typeof import('react-native')>();
  return {
  ...native,
  View: ({ onLayout, testID, ...props }: import('react-native').ViewProps) => {
    if (onLayout && testID) layouts.set(testID, onLayout);
    return createElement(native.View, { ...props, onLayout, ...{ 'data-testid': testID } });
  },
  AppState: {
    get currentState() { return appLifecycle.currentState; },
    addEventListener: (_type: string, listener: (state: import('react-native').AppStateStatus) => void) => {
      appLifecycle.listeners.add(listener);
      return { remove: () => { appLifecycle.listeners.delete(listener); } };
    },
  },
  AccessibilityInfo: { sendAccessibilityEvent: vi.fn() },
  Pressable: ({ accessibilityState, accessibilityRole, accessibilityLabel, accessibilityHint, onPress, children, disabled, ref, style }: {
    accessibilityState?: { disabled?: boolean; busy?: boolean; expanded?: boolean; selected?: boolean }; accessibilityRole?: string;
    accessibilityLabel?: string; onPress?: () => void; children: ReactNode; disabled?: boolean;
    accessibilityHint?: string; ref?: import('react').Ref<HTMLButtonElement>; style?: unknown;
  }) => createElement('button', {
    ref, role: accessibilityRole, 'aria-label': accessibilityLabel, title: accessibilityHint, onClick: onPress, disabled,
    style: [typeof style === 'function' ? style({ pressed: false }) : style].flat(Infinity)
      .reduce<Record<string, unknown>>((all, part) => (part && typeof part === 'object' ? { ...all, ...part } : all), {}),
    'aria-disabled': accessibilityState?.disabled, 'aria-busy': accessibilityState?.busy,
    'aria-expanded': accessibilityState?.expanded, 'aria-selected': accessibilityState?.selected,
  }, children),
  };
});

// A tela monta o painel da fila e o passa ao Composer; aqui ele vem vazio.
const noQueue: QueuePanel = { entries: [], actions: false };
const focusNavigation = vi.hoisted(() => ({
  listeners: new Map<string, Set<(event: { data: { closing: boolean } }) => void>>(),
  focused: true,
}));
const nativeNavigation = {
  isFocused: () => focusNavigation.focused,
  addListener: (name: string, listener: (event: { data: { closing: boolean } }) => void) => {
    const listeners = focusNavigation.listeners.get(name) ?? new Set();
    listeners.add(listener);
    focusNavigation.listeners.set(name, listeners);
    return () => { listeners.delete(listener); };
  },
};
const routerPush = vi.hoisted(() => vi.fn());
const chatRow = vi.hoisted(() => ({ extra: {} as Record<string, unknown> }));
const navigation = vi.hoisted(() => ({ back: vi.fn(), replace: vi.fn(), dismissTo: vi.fn(), canGoBack: true }));
const route = vi.hoisted(() => ({ params: { server: 's1', name: 'sess' }, segments: ['s'] }));
const realFirstInput = vi.hoisted(() => ({ enabled: false, send: vi.fn(), steer: vi.fn(), history: vi.fn(), transcribe: vi.fn(), relimpar: vi.fn(),
  upload: vi.fn(async () => ({ path: '/up/sess/ditado-1.m4a' })) }));
const voiceInput = vi.hoisted(() => ({ recording: false, simulateRecorder: false, delayStop: false, stop: vi.fn(),
  startPending: null as Promise<void> | null, startError: null as Error | null, stopWithoutResult: false,
  stopPending: null as Promise<void> | null, finishStop: null as null | (() => void), finished: null as Promise<void> | null,
  onFim: null as null | ((file: File, reason: MotivoFim, uri: string) => Promise<void>),
  finishRecording: null as null | ((reason: MotivoFim) => void), onStopError: null as null | ((e: Error) => void) }));
vi.mock('expo-router', () => ({
  useNavigation: () => nativeNavigation,
  useRouter: () => ({ push: routerPush, back: navigation.back, replace: navigation.replace, dismissTo: navigation.dismissTo, canGoBack: () => navigation.canGoBack }),
  useLocalSearchParams: () => route.params, useSegments: () => route.segments,
}));
vi.mock('@hangar/core', async (original) => ({
  ...await original<typeof import('@hangar/core')>(),
  fetchSessionsForServer: vi.fn(async () => [{ name: 'sess', provider: 'claude', ...chatRow.extra }]),
  fetchDictationOrganization: vi.fn(async () => 'on'),
  loadLiveVoiceSettings: vi.fn(async () => ({ enabled: false, codex: false, jev: false, voices: [],
    settings: { voice: null, codex_account: 'default', organizer: {
      direct: { model: null, effort: 'medium', tier: null }, plan: { model: null, effort: 'high', tier: null },
    } }, call: { active: false, client: null },
  })),
  interrupt: vi.fn(async () => {}),
  recarregarSessao: vi.fn(async () => ({ ok: true })),
  implementCodexPlan: vi.fn(async () => {}),
  setPermissionMode: vi.fn(async () => ({ mode: 'acceptEdits', current: 'acceptEdits', previous_non_plan: 'acceptEdits' })),
  sendInputForServer: realFirstInput.send,
  steerSession: realFirstInput.steer,
  getHistory: realFirstInput.history,
  transcribeUploadedForServer: realFirstInput.transcribe,
  // O ditado sobe o áudio por aqui (`audioOnly`) antes de transcrever.
  uploadFileForServer: realFirstInput.upload,
  relimparDitado: realFirstInput.relimpar,
}));
vi.mock('../stores/servers', () => {
  const state = { ready: true, activeId: 's2', servers: [{ id: 's1' }, { id: 's2' }], ensureActive: () => true, ensureOwnActive: () => {} };
  return { useServers: Object.assign((select: (s: typeof state) => unknown) => select(state), { getState: () => state }) };
});
vi.mock('../ui/Screen', () => ({ Screen: ({ children }: { children: ReactNode }) => createElement('div', null, children) }));
// A gaveta puxa o gesture-handler nativo, que o vitest não carrega; aqui só o conteúdo importa.
vi.mock('../features/sessions/SessionsDrawer', () => ({ SessionsDrawer: ({ children, onClose, onNewConversation }: { children: ReactNode; onClose: () => void; onNewConversation?: () => void }) => createElement('div', null, children,
  createElement('button', { 'aria-label': 'drawer-new-conversation', onClick: onNewConversation ?? onClose }),
  createElement('button', { 'aria-label': 'drawer-navigation-close', onClick: onClose })) }));
vi.mock('../features/sessions/ServerSheet', () => ({ ServerSheet: () => null }));
vi.mock('react-native-keyboard-controller', () => ({
  KeyboardAvoidingView: ({ children }: { children: ReactNode }) => createElement('div', null, children),
  // O KeyboardBar do chat lê o estado do teclado; nestes testes ele está sempre fechado.
  useKeyboardState: <T,>(select: (s: { isVisible: boolean; height: number }) => T) => select({ isVisible: false, height: 0 }),
}));
vi.mock('./LoopChip', () => ({ LoopChip: () => null }));
vi.mock('./OrqFooter', () => ({ OrqFooter: () => null }));
vi.mock('./TuiPill', () => ({ TuiPill: () => null }));
vi.mock('./RecarregarPill', () => ({ RecarregarPill: () => null }));
vi.mock('./PendingPlan', () => ({ PendingPlan: () => null }));
vi.mock('./SessionProblem', () => ({ SessionProblem: () => null }));
vi.mock('./SessionPickerSheet', () => ({ SessionPickerSheet: () => null }));
vi.mock('../features/create/CreateSessionSheet', () => ({ CreateSessionSheet: () => createElement('div', { 'data-create': true }) }));
vi.mock('../ui/Icon', () => ({ Icon: () => null }));
vi.mock('../ui/HangarMark', () => ({ HangarMark: () => null }));
const sheetEvents = vi.hoisted(() => ({ presented: undefined as (() => void) | undefined, dismissed: undefined as (() => void) | undefined }));
vi.mock('../ui/Sheet', () => ({ Sheet: ({ children, onDidPresent, onDismiss }: { children: ReactNode; onDidPresent?: () => void; onDismiss?: () => void }) => {
  sheetEvents.presented = onDidPresent;
  sheetEvents.dismissed = onDismiss;
  return createElement('div', null, children);
} }));
const panelEvents = vi.hoisted(() => ({ dismissed: undefined as (() => void) | undefined }));
vi.mock('../ui/AnchoredPanel', () => ({ AnchoredPanel: ({ open, onDismissed, children }: { open: boolean; onDismissed?: () => void; children: ReactNode }) => {
  panelEvents.dismissed = onDismissed;
  return open ? createElement('div', null, children) : null;
} }));
// A folha de anexo também é um Sheet; fora daqui para o `sheetEvents` seguir preso à folha de opções.
vi.mock('../ui/AttachSheet', () => ({ AttachSheet: ({ open, sendToGroup }: {
  open: boolean; sendToGroup?: { onPress: () => void };
}) => open && sendToGroup ? createElement('button', { 'aria-label': 'send-to-group', onClick: sendToGroup.onPress }) : null }));
vi.mock('../features/usage/HomeUsageCard', () => ({ HomeUsageCard: () => null, TextTabs: () => null }));
vi.mock('./ContextRing', () => ({ ContextRing: () => null }));
// Cada chave devolve o próprio nome, como nos outros testes de componente do app.
vi.mock('../paraglide/messages', () => ({ ...Object.fromEntries(
  ('arq_aba askq_sua_resposta bastao_dossie_sub bastao_dossie_titulo chat_voltar_sessoes codex_limites_titulo compartilhar_menu ctx_anexos ctx_atividade ctx_grupo ctx_limites ctx_repositorio ctx_terminal modo_so_ociosa more_fotos_videos_arquivos more_tarefas_agentes mods_celular_mostrar mods_celular_mostrar_um mods_celular_mostrar_paineis mods_celular_ocultar mods_celular_sub plugin_band_label navbar_mais_acoes par_titulo recarregar_sessao recarregar_sessao_detalhe sessao_trocar_de term_titulo '
    + 'askq_enviando board_arquivo board_imagem board_remover_anexo codex_orientar composer_anexar_arquivo composer_desfazer_limpeza composer_ditado_limpo composer_enviando_cancelar composer_enviar_mensagem composer_fila_acao composer_fila_aria composer_fila_contagem composer_gravando_audio composer_gravar_audio composer_mandando_grupo composer_mandar_grupo composer_mandar_tambem composer_mensagem composer_parar composer_parar_gravacao composer_pro_grupo composer_pros_dois composer_sessao_trabalhando composer_transcrevendo_audio composer_transcrever_de_novo')
    .concat(' live_voice_open composer_mic_style_hint composer_dictation_style composer_session_settings composer_session_settings_hint composer_steer_current_text ditado_estilo_titulo uso_aria codex_orientar_ajuda composer_fila_titulo composer_submission_recover_first queue_steer_uncertain')
    .concat(' composer_interromper_claude composer_interromper_msg composer_interromper comum_cancelar')
    .concat(' cite_action cite_too_long_aria selection_copy selection_copy_markdown selection_copy_image_url selection_copy_image_urls')
    .concat(' permissao_pedido comum_cancelar msg_aria_mensagens chat_plan_proposto composer_falha_envio nova_conversa_envio_incerto nova_conversa_resultado_salvar_erro nova_conversa_salvar_erro')
    .concat(' askq_enviando board_falha_envio board_falha_upload chat_chegou_mas chat_envio_incerto chat_nao_chegou_em chat_servidor_removido codex_orientar_recebido codex_orientar_sem_envio composer_ditado_anterior composer_ditado_aplicado composer_ditado_indisponivel composer_ditado_interrompido composer_ditado_recuperavel composer_draft_read_again composer_draft_recover_attach_busy composer_falha_gravacao composer_falha_transcricao composer_fila_erro composer_sem_acesso_fotos composer_sem_acesso_mic composer_submission_check composer_submission_rejected composer_submission_sending composer_transcrever_de_novo composer_transcricao_vazia composer_aguarde_transcricao')
    .concat(' draft_read_error draft_invalid draft_write_error draft_clear_error composer_draft_previous composer_draft_recover composer_draft_discard composer_draft_read_again')
    .concat(' composer_ditado_cru composer_ditado_versao composer_ditado_trocando native_dictation_draft_changed composer_voice_send_cancelled composer_voice_starting native_queue_discard_message')
    .concat(' sessao_nova nova_conversa_placeholder nova_conversa_sem_destino nova_conversa_opcoes nova_conversa_opcoes_fechar nova_conversa_destino_hint nova_conversa_config_hint nova_conversa_enviar criar_criando')
    .concat(' composer_mensagem_para comandos_titulo native_new_chat_title native_empty_chat_hint criar_mais_opcoes')
    .concat(' uso_titulo uso_vazio uso_secao_cota uso_secao_conversa uso_secao_numeros uso_statusline uso_custo uso_tempo_sessao uso_linha_projeto uso_reset composer_modelo ctx_contexto stats_faixa_aria')
    // Telas do redesign (Nova conversa, folha de anexo, lista de tarefas, uso na inicial).
    .concat(' ask_perguntas chat_abrir_terminal_codex chat_carregando_historico chat_codex_buffering chat_historico_antigo chat_sem_historico_anterior chat_sem_thread_codex chat_sem_thread_codex_hint chat_sessao_encerrada chat_sse_recusado chat_sse_tentar composer_adicionar_ao_chat composer_adicionar_arquivos composer_camera composer_fotos composer_sem_acesso_camera comum_falha_envio_opcao comum_voltar criar_retomar ctx_anexos_da_sessao')
    .concat(' home_usage_30d home_usage_7d home_usage_active_days home_usage_activity home_usage_all home_usage_cost home_usage_day home_usage_empty home_usage_load_failed home_usage_method home_usage_model_count home_usage_models home_usage_overview home_usage_partial home_usage_period_unsupported home_usage_sessions home_usage_today home_usage_tokens home_usage_top_model home_usage_warming home_usage_warming_timeout lista_tentar_novamente')
    .concat(' native_action_uncertain native_ask_fallback native_close native_dictation_active native_dictation_cancel native_dictation_level native_loading native_new_chat_folder native_tasks_all_done native_tasks_completed native_tasks_expand native_tasks_in_progress native_tasks_left native_tasks_left_1 native_tasks_minimize native_tasks_pending native_tasks_untitled native_thinking native_tools_failed native_tools_failed_1 native_tree_thoughts')
    .concat(' new_conversation_no_received_messages new_conversation_received_messages notice_compacted notice_hook_prompt notice_interrupted notice_skill_loaded nova_conversa_abrir nova_conversa_adotar nova_conversa_conferir nova_conversa_descartar nova_conversa_guardada nova_conversa_reenviar')
    .concat(' native_working_line pensamento_vivo tool_executando_ha estado_em_execucao atividade_subagente tool_abrir_agente tool_fase_escrevendo')
    .concat(' native_par_so_leitura chat_plan_implementar chat_plan_iniciando chat_plan_continuar chat_plan_erro chat_plan_indisponivel chat_plan_pedido native_plan_send_failed_stuck')
    .concat(' native_delivery_failed native_delivery_unproved native_queue_held native_no_safe_delivery native_delivering native_queued native_queue_discard msg_nao_chegou_reenvie')
    // Painel "Na fila" e a edição da mensagem da fila no Composer.
    .concat(' queue_panel_title queue_panel_collapse queue_panel_expand queue_panel_actions queue_panel_move_up queue_panel_move_down queue_panel_remove atalhos_editar comum_apagar')
    .concat(' queue_panel_update queue_panel_update_aria queue_panel_editing queue_entry_gone')
    .concat(' stats_cache stats_chamadas stats_chamadas_1 stats_io stats_llm stats_toks stats_toks_now stats_toks_recent stats_tools stats_ttft stats_turnos stats_turnos_1 sync_retry uso_janela_30d uso_janela_5h uso_janela_7d').split(' ').map((k) => [k, () => k]),
), queue_panel_steer_message: ({ text }: { text: string }) => `queue_panel_steer_message: ${text}` }));

// Rascunho em memória no lugar do MMKV; cada teste começa sem nada guardado.
const storage = vi.hoisted(() => ({ memory: new Map<string, string>(), failSet: false, failRemove: false }));
vi.mock('react-native-mmkv', () => ({
  createMMKV: () => ({
    getString: (key: string) => storage.memory.get(key),
    set: (key: string, value: string) => {
      if (storage.failSet) throw new Error('storage unavailable');
      storage.memory.set(key, value);
    },
    remove: (key: string) => {
      if (storage.failRemove) throw new Error('storage unavailable');
      storage.memory.delete(key);
    },
    getNumber: () => undefined, getBoolean: () => undefined, contains: (key: string) => storage.memory.has(key),
  }),
}));
beforeEach(() => {
  storage.memory.clear(); storage.failSet = false; storage.failRemove = false; sessionsState.rows = []; realFirstInput.enabled = false; voiceInput.simulateRecorder = false;
  voiceInput.startPending = null; voiceInput.startError = null; voiceInput.stopWithoutResult = false;
  realFirstInput.upload.mockReset().mockResolvedValue({ path: '/up/sess/ditado-1.m4a' });
  _limparEsfriamentoParaTestes();
  _resetRotasForTests();
});
afterEach(() => { _limparEsfriamentoParaTestes(); _resetRotasForTests(); });

const sessionsState = vi.hoisted(() => ({ rows: [] as { serverId: string; name: string; jsonl?: string | null; pair_peers?: string[] }[], byServerRecord: {} }));

// Composer isolado: sem picker, pills, ditado nem store real — só o que decide o botão Parar.
const composerChat = vi.hoisted(() => ({ state: 'idle' as string, send: vi.fn(async (_text: string) => {}) }));
vi.mock('expo-image-picker', () => ({}));
vi.mock('@react-native-menu/menu', () => ({ MenuView: ({ children }: { children: ReactNode }) => createElement('div', null, children) }));
vi.mock('expo-document-picker', () => ({}));
vi.mock('./draftAttachments', () => ({
  retainDraftAttachment: vi.fn(async (attachment: import('../stores/drafts').DraftAttachment) => attachment),
  removeDraftAttachment: vi.fn(),
}));
vi.mock('../ui/Glass', () => ({ Glass: ({ children }: { children: ReactNode }) => createElement('div', null, children) }));
vi.mock('../ui/MultilineInput', () => ({ MultilineInput: ({ value, onChangeText, accessibilityLabel, ref }: {
  value: string; onChangeText: (text: string) => void; accessibilityLabel?: string; ref?: import('react').Ref<HTMLTextAreaElement>;
}) => createElement('textarea', { ref, 'aria-label': accessibilityLabel, value, readOnly: true, onInput: (e: { currentTarget: { value: string } }) => onChangeText(e.currentTarget.value) }),
}));
vi.mock('../features/pills/PillMenu', () => ({ PillMenu: () => null }));
vi.mock('../features/ditado/EstiloPill', () => ({ DictationStyleMenu: () => null, useDictationStyleLabel: () => 'estilo' }));
vi.mock('./SessionSettings', () => ({ SessionSettingsButton: () => createElement('button', { 'aria-label': 'composer_session_settings' }) }));
vi.mock('../features/ditado/useDitado', () => ({ useDitado: (callbacks: {
  onFim: NonNullable<typeof voiceInput.onFim>; onErroParada?: (e: Error) => void;
}) => {
  const [recording, setRecording] = useState(false);
  voiceInput.onFim = callbacks.onFim;
  voiceInput.onStopError = callbacks.onErroParada ?? null;
  const finishRecording = (reason: MotivoFim) => {
    setRecording(false);
    voiceInput.finished = callbacks.onFim(new File(['audio'], 'ditado.m4a', { type: 'audio/m4a' }), reason, 'file:///cache/ditado.m4a');
  };
  voiceInput.finishRecording = finishRecording;
  return { gravando: voiceInput.recording || recording, rms: 0,
    iniciar: async () => {
      if (voiceInput.startPending) await voiceInput.startPending;
      if (voiceInput.startError) throw voiceInput.startError;
      if (voiceInput.simulateRecorder) setRecording(true);
    },
    parar: async (reason: MotivoFim) => {
      voiceInput.stop(reason);
      if (!voiceInput.simulateRecorder || !recording) return;
      if (voiceInput.stopWithoutResult) { setRecording(false); return; }
      if (voiceInput.stopPending) return voiceInput.stopPending;
      if (voiceInput.delayStop) {
        voiceInput.stopPending = new Promise<void>((resolve) => {
          voiceInput.finishStop = () => {
            finishRecording(reason);
            voiceInput.stopPending = null;
            resolve();
          };
        });
        return voiceInput.stopPending;
      }
      // Como o gravador real: parar termina sem aguardar a transcrição de onFim.
      finishRecording(reason);
    },
  };
} }));
vi.mock('../features/ditado/ditadoEstiloStore', () => ({ dictationStyleStore: () => ({ getState: () => ({ pronto: false }) }) }));
vi.mock('./CommandSheet', () => ({
  CommandSheet: () => null,
  useSessionCommands: () => ({ commands: [], error: '', retry: () => {} }),
}));
vi.mock('./SlashSuggest', () => ({ SlashSuggest: () => null }));
vi.mock('./SideQuestionSheet', () => ({ SideQuestionSheet: () => null }));
vi.mock('../stores/sessions', () => ({
  useSessions: Object.assign((sel: (s: unknown) => unknown) => sel(sessionsState), { getState: () => sessionsState }),
}));
vi.mock('../stores/chat', async (original) => {
  const actual = await original<typeof import('../stores/chat')>();
  const snap = () => ({ pending: [], events: [], stateEvent: { state: composerChat.state } });
  const use = Object.assign((sel: (s: unknown) => unknown) => sel(snap()), {
    getState: snap, setState: () => {}, subscribe: () => () => {},
  });
  return { ...actual,
    chatStore: (serverId: string, name: string) => realFirstInput.enabled ? actual.chatStore(serverId, name)
      : { use, send: composerChat.send, retain: () => {}, release: () => {}, retry: () => {},
        startQueueEdit: (_id: string, draft: string) => draft, cancelQueueEdit: () => {}, dropQueueEdit: () => {},
        submitQueueEdit: async () => {}, removeQueued: async () => {}, steerQueued: async () => {}, moveQueued: async () => {} },
    filaCount: (...args: Parameters<typeof actual.filaCount>) => realFirstInput.enabled ? actual.filaCount(...args) : 0,
    isSubmitting: (serverId: string, name: string) => realFirstInput.enabled && actual.isSubmitting(serverId, name),
  };
});

const firstInput = vi.hoisted(() => ({
  attempt: null as FirstConversationAttempt | null,
  send: vi.fn<(serverId: string, id: string) => Promise<void>>(),
  confirm: vi.fn(),
}));
vi.mock('../stores/newConversation', async (original) => {
  const actual = await original<typeof import('../stores/newConversation')>();
  return { ...actual,
    useNewConversation: Object.assign((select: (state: ReturnType<typeof actual.useNewConversation.getState>) => unknown) => realFirstInput.enabled ? actual.useNewConversation(select) : select({
      attempts: firstInput.attempt ? { s1: firstInput.attempt } : {}, issues: {}, busy: {},
    }), { getState: () => realFirstInput.enabled ? actual.useNewConversation.getState()
      : { attempts: firstInput.attempt ? { s1: firstInput.attempt } : {}, issues: {}, busy: {} } }),
    readFirstInput: (serverId: string, name: string) => realFirstInput.enabled ? actual.readFirstInput(serverId, name) : firstInput.attempt?.serverId === serverId
      && firstInput.attempt.sessionName === name ? firstInput.attempt : null,
    confirmFirstInput: (id: string) => realFirstInput.enabled ? actual.confirmFirstInput(id) : firstInput.confirm(id),
    sendFirstInput: (serverId: string, id: string) => realFirstInput.enabled ? actual.sendFirstInput(serverId, id) : firstInput.send(serverId, id),
    restoreAttempt: (serverId: string) => realFirstInput.enabled ? actual.restoreAttempt(serverId) : null,
  };
});

// Lista isolada: a bolha só registra o texto recebido, pra provar o que chega nela.
const bubbleTexts = vi.hoisted(() => [] as string[]);
vi.mock('./AssistantBubble', () => ({ AssistantBubble: ({ text }: { text: string }) => { bubbleTexts.push(text); return null; }, mkMarkdownStyle: () => ({}) }));
vi.mock('./UserBubble', () => ({ UserBubble: () => null }));
// Anexos da bolha puxam módulos nativos (gesture-handler, expo-audio, expo-file-system) que o node não carrega.
vi.mock('./ImageThumb', () => ({ ImageThumb: () => null }));
vi.mock('./AudioChip', () => ({ AudioChip: ({ uri }: { uri: string }) => createElement('span', { 'data-audio': uri }) }));
vi.mock('../features/attachments/DocumentViewer', () => ({ DocumentViewer: () => null }));
vi.mock('../features/attachments/mediaCache', () => ({ canShareFile: false, shareFile: async () => {} }));
vi.mock('../ui/Toast', () => ({ toast: { ok: () => {}, erro: () => {} } }));
vi.mock('expo-clipboard', () => ({ setStringAsync: vi.fn(async () => {}) }));
vi.mock('./PreviewBubble', () => ({ PreviewBubble: () => null }));
vi.mock('./ThinkingBlock', () => ({ ThinkingBlock: () => null }));
vi.mock('./tools/ToolCard', () => ({ ToolCard: () => null }));
vi.mock('./tools/ToolGroup', () => ({ ToolGroup: () => null }));
vi.mock('./tools/ToolDetailSheet', () => ({ ToolDetailSheet: () => null }));
vi.mock('../stores/aparencia', () => ({ useAparencia: (sel: (s: unknown) => unknown) => sel({ pensamentoTools: 'busca' }) }));
vi.mock('@legendapp/list/react-native', () => ({
  LegendList: ({ data, renderItem, keyExtractor, ListFooterComponent }: {
    data: unknown[]; renderItem: (a: { item: unknown }) => ReactNode; keyExtractor: (i: unknown) => string; ListFooterComponent?: ReactNode;
  }) => createElement('div', null, ...data.map((item) => createElement('div', { key: keyExtractor(item) }, renderItem({ item }))), createElement('div', { 'data-testid': 'message-footer' }, ListFooterComponent)),
}));

vi.mock('react-native-enriched-markdown', () => ({
  EnrichedMarkdownText: ({ markdown }: { markdown: string }) => createElement('pre', null, markdown),
}));
vi.mock('./TableChart', () => ({ TableChart: () => null }));
vi.mock('./tableChartPref', () => ({ getTableChartPref: () => 'table', setTableChartPref: () => {} }));
vi.mock('./BubbleActions', () => ({ BubbleActions: () => null, pararTts: () => {} }));
vi.mock('./ArquivoChip', () => ({
  ArquivoChip: ({ caminho, onPress }: { caminho: string; onPress: () => void }) =>
    createElement('button', { onClick: onPress, 'aria-label': caminho }, caminho),
}));

import { ChatHeader } from './ChatHeader';
import { MessageList } from './MessageList';
import { MoreSheet } from './MoreSheet';
import { Composer } from './Composer';
import { OptionButtons } from './OptionButtons';
import ChatScreen from '../../app/s/[server]/[name]/index';
import CreateRoute from '../../app/create';
import { NewConversation } from '../features/create/NewConversation';
import { _resetNewConversationForTests, recoverAttempt, restoreAttempt, useNewConversation } from '../stores/newConversation';
import { AccessibilityInfo, Alert } from 'react-native';
import { retainDraftAttachment } from './draftAttachments';
import { chatStore, _resetChatsForTests } from '../stores/chat';
import { useServers } from '../stores/servers';
import { useMods } from '../stores/mods';
import { useLiveVoice } from '../features/voice/liveVoiceStore';
import { readDictation, readDraft, setDictationInFlight, writeDictation, writeDraft } from '../stores/drafts';

async function render(el: ReturnType<typeof createElement>) {
  const container = document.createElement('div');
  const root = createRoot(container);
  await act(async () => root.render(el));
  return { container, root };
}
const button = (container: HTMLElement, label: string) => [...container.querySelectorAll('button, [role="button"]')]
  .find((el) => el.textContent === label || el.getAttribute('aria-label') === label) as HTMLElement | undefined;

const header = { name: 'g1-orq', state: null, onBack: () => {}, onMore: () => {}, onTitlePress: () => {} };
const sheet = { open: true, onClose: () => {}, serverId: 's1', name: 'g1-orq' };

it.each([0, 1])('opção %s e cancelar compartilham trava, mantendo índice 1-based e liberando no erro', async (selected) => {
  let fail!: (reason: Error) => void;
  const onSelect = vi.fn(() => new Promise<void>((_, reject) => { fail = reject; }));
  const onCancel = vi.fn(async () => {});
  const { container, root } = await render(createElement(OptionButtons, {
    question: 'permission', options: ['Yes', 'No'], interactionId: 'perm:1', onSelect, onCancel,
  }));
  const buttons = container.querySelectorAll<HTMLButtonElement>('button');
  act(() => { buttons[selected].click(); buttons[1 - selected].click(); buttons[2].click(); });
  // A escolha leva o ID do cartão em que a pessoa agiu.
  expect(onSelect).toHaveBeenCalledExactlyOnceWith(selected + 1, 'perm:1');
  expect(onCancel).not.toHaveBeenCalled();
  expect([...buttons].every((b) => b.disabled)).toBe(true);
  await act(async () => fail(new Error('recusado')));
  await act(async () => buttons[2].click());
  expect(onCancel).toHaveBeenCalledTimes(1);
  act(() => root.unmount());
});

describe('ações e atualização da conversa na rota', () => {
  const lifecycle = (state: import('react-native').AppStateStatus) => act(() => {
    appLifecycle.currentState = state;
    appLifecycle.listeners.forEach((listener) => listener(state));
  });
  beforeEach(() => {
    _resetChatsForTests();
    realFirstInput.enabled = true;
    realFirstInput.history.mockReset().mockResolvedValue([]);
    realFirstInput.send.mockReset().mockResolvedValue(undefined);
    route.params = { server: 's1', name: 'sess' };
    firstInput.attempt = null;
    appLifecycle.currentState = 'active';
    chatRow.extra = {};
    vi.mocked(fetchSessionsForServer).mockClear();
    vi.mocked(interrupt).mockReset().mockResolvedValue(undefined);
    vi.mocked(recarregarSessao).mockClear();
    vi.mocked(implementCodexPlan).mockClear();
    vi.mocked(setPermissionMode).mockClear();
    configureApi({ getBaseUrl: () => 'http://unused', getToken: () => '', onUnauthorized: () => {}, origin: null,
      createEventSource: () => ({ addEventListener: () => {}, removeEventListener: () => {}, close: () => {},
        onerror: null, onopen: null, readyState: 1 }) });
  });
  afterEach(() => { _resetChatsForTests(); chatRow.extra = {}; vi.useRealTimers(); });

  it('Nova conversa da gaveta do chat pede compose na inicial, sem mudar o fechamento das demais navegações', async () => {
    navigation.dismissTo.mockClear();
    const { container, root } = await render(createElement(ChatScreen));
    try {
      await act(async () => button(container, 'drawer-new-conversation')!.click());
      expect(navigation.dismissTo).toHaveBeenLastCalledWith({ pathname: '/', params: { compose: '1' } });
      await act(async () => button(container, 'drawer-navigation-close')!.click());
      expect(navigation.dismissTo).toHaveBeenLastCalledWith('/');
    } finally { act(() => root.unmount()); }
  });

  it('lista reserva o espaço real dos mods e volta ao vidro do composer quando eles saem', async () => {
    layouts.clear();
    const { container, root } = await render(createElement(ChatScreen));
    const measure = (id: string, height: number) => act(() => layouts.get(id)!({ nativeEvent: { layout: { x: 0, y: 0, width: 390, height } } } as import('react-native').LayoutChangeEvent));
    const viewport = () => container.querySelector<HTMLElement>('[data-testid="chat-viewport"]')!;
    const footerHasHeight = (height: number) => [...container.querySelectorAll<HTMLElement>('[data-testid="message-footer"] div')].some(el => el.style.height === `${height}px`);
    try {
      measure('chat-dock', 120);
      expect(viewport().style.marginBottom).toBe('-100px');
      expect(footerHasHeight(120)).toBe(true);
      act(() => {
        useMods.setState({ visible: true });
        chatStore('s1', 'sess').use.setState({ pluginUi: {
          above: { type: 'Box', props: { flexDirection: 'column' }, children: Array.from({ length: 5 }, (_, i) => ({ type: 'Text', children: [`mod-${i}`] })) },
          panes: [], shownId: null, source: 'surface', columns: 110,
        } });
      });
      measure('mods-dock', 121);
      measure('chat-dock', 241);
      expect(viewport().style.marginBottom).toBe('0px');
      expect(viewport().style.overflow).toBe('hidden');
      expect(footerHasHeight(241)).toBe(false);
      expect(footerHasHeight(120)).toBe(false);
      act(() => useMods.setState({ visible: false }));
      measure('mods-dock', 0);
      measure('chat-dock', 120);
      expect(viewport().style.marginBottom).toBe('-100px');
      expect(footerHasHeight(120)).toBe(true);
    } finally {
      act(() => root.unmount());
      useMods.setState({ visible: false });
      layouts.clear();
    }
  });

  it.each([true, false])('cancelar opções só devolve pendente depois de interrupt aceito: %s', async (accepted) => {
    let finish!: () => void;
    vi.mocked(interrupt).mockImplementationOnce(() => new Promise<void>((resolve, reject) => {
      finish = () => accepted ? resolve() : reject(Object.assign(new Error('409: recusado'), { status: 409 }));
    }));
    const { container, root } = await render(createElement(ChatScreen));
    const chat = chatStore('s1', 'sess');
    const pending = [{ id: 'pending-a', text: 'anterior' }, { id: 'pending-b', text: 'devolver' }];
    act(() => chat.use.setState({ pending, stateEvent: {
      session: 'sess', state: 'awaiting_input', question: 'permission', options: ['Yes', 'No'], interaction_id: 'perm:1',
    } }));
    const field = container.querySelector('textarea')!;
    act(() => {
      Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value')!.set!.call(field, 'digitado');
      field.dispatchEvent(new Event('input', { bubbles: true }));
    });
    act(() => [...container.querySelectorAll<HTMLButtonElement>('button')].find((b) => b.textContent === '✕comum_cancelar')!.click());
    expect(interrupt).toHaveBeenCalledExactlyOnceWith('sess', true, useServers.getState().servers[0]);
    expect(field.value).toBe('digitado');
    expect(chat.use.getState().pending).toEqual(pending);
    await act(async () => finish());
    expect(field.value).toBe(accepted ? 'devolver\n\ndigitado' : 'digitado');
    expect(chat.use.getState().pending).toEqual(accepted ? [pending[0]] : pending);
    if (!accepted) expect(container.textContent).toContain('409: recusado');
    act(() => root.unmount());
  });

  it.each(['cancelar', 'Parar'])('%s não devolve ao campo pendente entregue antes do ACK da interrupção', async (action) => {
    let finish!: () => void;
    vi.mocked(interrupt).mockImplementationOnce(() => new Promise<void>((resolve) => { finish = resolve; }));
    const alert = vi.spyOn(Alert, 'alert').mockImplementation(() => {});
    const { container, root } = await render(createElement(ChatScreen));
    try {
      const chat = chatStore('s1', 'sess');
      const pending = [{ id: 'pending-a', text: 'anterior' }, { id: 'pending-b', text: 'já entregue' }];
      act(() => chat.use.setState({ pending, stateEvent: action === 'cancelar' ? {
        session: 'sess', state: 'awaiting_input', question: 'permission', options: ['Yes', 'No'], interaction_id: 'perm:1',
      } : { session: 'sess', state: 'working' } }));
      if (action === 'cancelar') {
        act(() => button(container, '✕comum_cancelar')!.click());
      } else {
        act(() => button(container, 'composer_parar')!.click());
        const choices = alert.mock.calls[0][2] as { style?: string; onPress?: () => void }[];
        act(() => choices.find((choice) => choice.style === 'destructive')!.onPress!());
      }
      expect(interrupt).toHaveBeenCalledExactlyOnceWith('sess', true, useServers.getState().servers[0]);
      // Parar exige campo vazio; a edição ocorre enquanto a interrupção aguarda resposta.
      const field = container.querySelector('textarea')!;
      act(() => {
        Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value')!.set!.call(field, 'digitado');
        field.dispatchEvent(new Event('input', { bubbles: true }));
      });
      expect(field.value).toBe('digitado');
      expect(chat.use.getState().pending).toEqual(pending);
      // A entrega pelo SSE retira só a última mensagem, antes da confirmação do interrupt.
      act(() => chat.use.setState((live) => ({ pending: live.pending.filter((p) => p.id !== pending[1].id) })));
      expect(chat.use.getState().pending).toEqual([pending[0]]);
      await act(async () => finish());
      expect(field.value).toBe('digitado');
      expect(chat.use.getState().pending).toEqual([pending[0]]);
    } finally {
      act(() => root.unmount());
      alert.mockRestore();
    }
  });

  it.each(['recarregar', 'codex', 'claude'])('%s usa o servidor da rota e conserva o destino na reversão', async (action) => {
    chatRow.extra = { provider: action === 'codex' ? 'codex' : 'claude' };
    const { container, root } = await render(createElement(ChatScreen));
    const target = useServers.getState().servers[0];
    const chat = chatStore('s1', 'sess');
    act(() => chat.use.setState({ stateEvent: {
      session: 'sess', state: 'idle', headless: true, claude_permission_mode: 'plan', claude_previous_non_plan: 'acceptEdits',
    }, events: [{ id: 'plan:1', kind: 'assistant_msg', text: action === 'codex' ? '<proposed_plan>\n# Plano\n</proposed_plan>' : '# Plano' }] }));
    if (action === 'recarregar') {
      await act(async () => container.querySelector<HTMLButtonElement>('[aria-label="recarregar_sessao"]')!.click());
      expect(recarregarSessao).toHaveBeenCalledExactlyOnceWith('sess', target);
    } else {
      if (action === 'claude') realFirstInput.send.mockImplementationOnce(async () => {
        useServers.getState().servers.reverse();
        throw Object.assign(new Error('409: recusado'), { status: 409 });
      });
      try {
        await act(async () => button(container, 'chat_plan_implementar')!.click());
        if (action === 'codex') expect(implementCodexPlan).toHaveBeenCalledExactlyOnceWith('sess', target);
        else expect(vi.mocked(setPermissionMode).mock.calls).toEqual([['sess', 'acceptEdits', target], ['sess', 'plan', target]]);
      } finally {
        if (action === 'claude') useServers.getState().servers.reverse();
      }
    }
    act(() => root.unmount());
  });

  it('polling limpa o timer no fundo, retoma uma vez e remove o listener ao desmontar', async () => {
    vi.useFakeTimers();
    chatRow.extra = { provider: 'codex', tracked: true };
    const listeners = appLifecycle.listeners.size;
    const { root } = await render(createElement(ChatScreen));
    expect(fetchSessionsForServer).toHaveBeenCalledTimes(1);
    lifecycle('background');
    await act(async () => vi.advanceTimersByTimeAsync(20_000));
    expect(fetchSessionsForServer).toHaveBeenCalledTimes(1);
    lifecycle('active');
    lifecycle('active');
    await act(async () => vi.advanceTimersByTimeAsync(0));
    expect(fetchSessionsForServer).toHaveBeenCalledTimes(2);
    await act(async () => vi.advanceTimersByTimeAsync(5000));
    expect(fetchSessionsForServer).toHaveBeenCalledTimes(3);
    act(() => root.unmount());
    expect(appLifecycle.listeners.size).toBe(listeners);
  });

  it.each([true, false])('resposta tardia após fundo, com sucesso: %s, não cria segundo polling na volta', async (success) => {
    vi.useFakeTimers();
    chatRow.extra = { provider: 'codex', tracked: true };
    let finish!: () => void;
    vi.mocked(fetchSessionsForServer).mockImplementationOnce(() => new Promise<SessionInfo[]>((resolve, reject) => {
      finish = () => success ? resolve([{ name: 'sess', provider: 'codex', tracked: true } as SessionInfo]) : reject(new Error('offline'));
    }));
    const { root } = await render(createElement(ChatScreen));
    lifecycle('background');
    lifecycle('active');
    await act(async () => finish());
    expect(fetchSessionsForServer).toHaveBeenCalledTimes(2);
    await act(async () => vi.advanceTimersByTimeAsync(5000));
    expect(fetchSessionsForServer).toHaveBeenCalledTimes(3);
    act(() => root.unmount());
  });

  it('montagem no fundo não consulta antes de o app voltar', async () => {
    appLifecycle.currentState = 'background';
    const { root } = await render(createElement(ChatScreen));
    expect(fetchSessionsForServer).not.toHaveBeenCalled();
    lifecycle('active');
    await act(async () => {});
    expect(fetchSessionsForServer).toHaveBeenCalledTimes(1);
    act(() => root.unmount());
  });
});

describe('terminal escondido', () => {
  it('cabeçalho não tem botão Terminal: ele mora só no "⋯"', async () => {
    const { container, root } = await render(createElement(ChatHeader, header));
    expect(container.querySelector('[aria-label="term_titulo"]')).toBeNull();
    expect(container.querySelector('[aria-label="navbar_mais_acoes"]')).not.toBeNull();
    act(() => root.unmount());
  });

  it('"⋯" do orquestrador não lista Terminal, resposta, anexos nem grupo', async () => {
    const { container, root } = await render(createElement(MoreSheet, { ...sheet, orq: true }));
    expect(container.querySelector('[aria-label="term_titulo"]')).toBeNull();
    expect(container.querySelector('[aria-label="par_titulo"]')).toBeNull();
    expect(container.querySelector('[aria-label="askq_sua_resposta"]')).toBeNull();
    expect(container.querySelector('[aria-label="ctx_anexos"]')).toBeNull();
    expect(container.querySelector('[aria-label="arq_aba"]')).not.toBeNull();
    act(() => root.unmount());
  });

  it('"⋯" de sessão comum continua listando Terminal, resposta e anexos', async () => {
    const { container, root } = await render(createElement(MoreSheet, { ...sheet, temPergunta: true }));
    expect(container.querySelector('[aria-label="term_titulo"]')).not.toBeNull();
    expect(container.querySelector('[aria-label="askq_sua_resposta"]')).not.toBeNull();
    expect(container.querySelector('[aria-label="ctx_anexos"]')).not.toBeNull();
    expect(container.querySelector('[aria-label="par_titulo"]')).not.toBeNull();
    act(() => root.unmount());
  });

  it('"⋯" sem pergunta pendente não oferece "Sua resposta"; sem pane não oferece Terminal', async () => {
    const { container, root } = await render(createElement(MoreSheet, { ...sheet, semTerminal: true }));
    expect(container.querySelector('[aria-label="askq_sua_resposta"]')).toBeNull();
    expect(container.querySelector('[aria-label="term_titulo"]')).toBeNull();
    expect(container.querySelector('[aria-label="ctx_anexos"]')).not.toBeNull();
    act(() => root.unmount());
  });
});

// A tela de verdade (CreateSessionSheet) entrega as pílulas; aqui bastam botões que chamam os ganchos.
let folderOpener: HTMLButtonElement | null = null;
const newConversationProps = (server: object) => ({
  server: server as never,
  destination: null, destinationPending: false, providerLabel: 'Codex',
  folderPanel: () => createElement('div', null, 'painel-pastas'),
  topPills: (openFolder: (opener: never) => void) => createElement('button', {
    ref: (el: HTMLButtonElement | null) => { folderOpener = el; },
    onClick: () => openFolder(folderOpener as never),
  }, 'repo'),
  bottomPills: null,
  providerChip: createElement('button', null, 'Codex'),
  resume: null, notices: null,
  body: { cwd: '/repo', provider: 'codex' as const }, blocked: false,
});

it('painel de pasta da Nova conversa devolve foco ao seletor e preserva o rascunho', async () => {
  firstInput.attempt = null;
  vi.mocked(AccessibilityInfo.sendAccessibilityEvent).mockClear();
  const { container, root } = await render(createElement(NewConversation, newConversationProps({ id: 's1' })));
  const field = container.querySelector('textarea')!;
  act(() => {
    field.value = 'primeira mensagem';
    field.dispatchEvent(new Event('input', { bubbles: true }));
  });
  expect(AccessibilityInfo.sendAccessibilityEvent).not.toHaveBeenCalled();
  // O painel de pasta devolve o foco à pílula que o abriu.
  const opener = button(container, 'repo')!;
  act(() => opener.click());
  expect(container.textContent).toContain('painel-pastas');
  act(() => panelEvents.dismissed?.());
  expect(AccessibilityInfo.sendAccessibilityEvent).toHaveBeenLastCalledWith(opener, 'focus');
  expect(AccessibilityInfo.sendAccessibilityEvent).toHaveBeenCalledTimes(1);
  expect(field.value).toBe('primeira mensagem');
  act(() => root.unmount());
});

describe('Parar no Composer', () => {
  const props = { serverId: 's1', name: 'sess', queuePanel: noQueue };

  it('ocioso: sem Parar, envio presente', async () => {
    composerChat.state = 'idle';
    const { container, root } = await render(createElement(Composer, { ...props, onStop: () => {} }));
    expect(container.querySelector('[aria-label="composer_parar"]')).toBeNull();
    expect(container.querySelector('[aria-label="composer_enviar_mensagem"]')).not.toBeNull();
    act(() => root.unmount());
  });

  it('trabalhando sem texto: Parar no lugar do envio, pede confirmação e só então chama onStop', async () => {
    composerChat.state = 'working';
    const onStop = vi.fn();
    const alert = vi.spyOn(Alert, 'alert');
    const { container, root } = await render(createElement(Composer, { ...props, onStop }));
    const parar = container.querySelector<HTMLButtonElement>('[aria-label="composer_parar"]');
    expect(parar).not.toBeNull();
    expect(container.querySelector('[aria-label="composer_enviar_mensagem"]')).toBeNull();
    act(() => parar!.click());
    expect(onStop).not.toHaveBeenCalled();
    const buttons = alert.mock.calls[0][2] as { style?: string; onPress?: () => void }[];
    act(() => buttons.find((b) => b.style === 'destructive')!.onPress!());
    expect(onStop).toHaveBeenCalledTimes(1);
    alert.mockRestore();
    act(() => root.unmount());
  });

  it('interrupção em voo: Parar desabilitado', async () => {
    composerChat.state = 'working';
    const { container, root } = await render(createElement(Composer, { ...props, onStop: () => {}, stopping: true }));
    expect(container.querySelector<HTMLButtonElement>('[aria-label="composer_parar"]')!.disabled).toBe(true);
    expect(container.querySelector('[aria-label="composer_parar"]')!.getAttribute('aria-busy')).toBe('true');
    act(() => root.unmount());
  });

  it('VoiceOver identifica o campo preenchido e o estado de envio; com texto não há Parar', async () => {
    composerChat.state = 'working';
    let finish!: () => void;
    composerChat.send.mockImplementationOnce(() => new Promise<void>((resolve) => { finish = resolve; }));
    const { container, root } = await render(createElement(Composer, { ...props, draft: 'texto', onStop: () => {} }));
    const send = container.querySelector<HTMLButtonElement>('[aria-label="composer_enviar_mensagem"]')!;
    expect(container.querySelector('[aria-label="composer_mensagem"]')).not.toBeNull();
    // Com texto a linha é Orientar + Enviar; o Parar volta quando o campo esvazia.
    expect(container.querySelector('[aria-label="composer_parar"]')).toBeNull();
    expect(send.getAttribute('aria-disabled')).toBe('false');
    act(() => send.click());
    expect(send.getAttribute('aria-disabled')).toBe('true');
    expect(send.getAttribute('aria-busy')).toBe('true');
    await act(async () => finish());
    expect(send.getAttribute('aria-busy')).toBe('false');
    act(() => root.unmount());
  });

  it('foco acessível volta ao campo só depois da folha sair, sem abrir teclado nem repetir no streaming', async () => {
    vi.mocked(AccessibilityInfo.sendAccessibilityEvent).mockClear();
    const { container, root } = await render(createElement(Composer, { ...props, draft: 'rascunho' }));
    const emit = (name: string, closing: boolean) => act(() => {
      focusNavigation.listeners.get(name)?.forEach((listener) => listener({ data: { closing } }));
    });
    emit('transitionEnd', false);
    expect(AccessibilityInfo.sendAccessibilityEvent).not.toHaveBeenCalled();
    focusNavigation.focused = false;
    emit('blur', true);
    emit('transitionEnd', true);
    expect(AccessibilityInfo.sendAccessibilityEvent).not.toHaveBeenCalled();
    focusNavigation.focused = true;
    emit('transitionEnd', false);
    expect(AccessibilityInfo.sendAccessibilityEvent).toHaveBeenCalledExactlyOnceWith(container.querySelector('textarea'), 'focus');
    await act(async () => root.render(createElement(Composer, { ...props, draft: 'rascunho' })));
    emit('transitionEnd', false);
    expect(AccessibilityInfo.sendAccessibilityEvent).toHaveBeenCalledTimes(1);
    act(() => root.unmount());
    expect([...focusNavigation.listeners.values()].every((listeners) => listeners.size === 0)).toBe(true);
  });

  it('sem onStop (quem não pode parar): nada de Parar mesmo trabalhando', async () => {
    composerChat.state = 'working';
    const { container, root } = await render(createElement(Composer, props));
    expect(container.querySelector('[aria-label="composer_parar"]')).toBeNull();
    act(() => root.unmount());
  });
});

describe('linha de botões do Composer', () => {
  it('voz ao vivo mora no topo, fora da linha de botões do Composer e da Nova conversa', async () => {
    composerChat.state = 'idle';
    firstInput.attempt = null;
    const { settingsByServer } = useLiveVoice.getState();
    useLiveVoice.setState({ settingsByServer: {} });
    vi.mocked(loadLiveVoiceSettings).mockResolvedValueOnce({
      enabled: true, codex: true, jev: true, voices: ['sol'],
      settings: { voice: 'sol', codex_account: 'default', organizer: {
        direct: { model: null, effort: 'medium', tier: null }, plan: { model: null, effort: 'high', tier: null },
      } }, call: { active: false, client: null },
    });
    const { container, root } = await render(createElement(Composer, { serverId: 's1', name: 'sess', queuePanel: noQueue }));
    try {
      await act(async () => useLiveVoice.getState().loadSettings(useServers.getState().servers.find(server => server.id === 's1')!));
      expect(container.querySelector('[aria-label^="live_voice_open"]')).toBeNull();
      const mic = button(container, 'composer_gravar_audio');
      const settings = button(container, 'composer_session_settings');
      const send = button(container, 'composer_enviar_mensagem');
      expect(button(container, 'composer_adicionar_ao_chat')).toBeDefined();
      expect(mic).toBeDefined();
      expect(settings).toBeDefined();
      expect(send).toBeDefined();
      expect(settings!.nextElementSibling).toBe(send);

      await act(async () => root.render(createElement(NewConversation, newConversationProps({ id: 's1' }))));
      expect(container.querySelector('[aria-label="nova_conversa_placeholder"]')).not.toBeNull();
      expect(button(container, 'composer_gravar_audio')).toBeDefined();
      expect(button(container, 'nova_conversa_enviar')).toBeDefined();
      expect(container.querySelector('[aria-label^="live_voice_open"]')).toBeNull();
    } finally {
      act(() => root.unmount());
      useLiveVoice.setState({ settingsByServer });
    }
  });

  it('campo em linha própria; +, microfone com a dica do estilo e um chip só de ajustes', async () => {
    composerChat.state = 'idle';
    const { container, root } = await render(createElement(Composer, { serverId: 's1', name: 'sess', queuePanel: noQueue }));
    const field = container.querySelector('[aria-label="composer_mensagem"]')!;
    const settings = container.querySelector('[aria-label="composer_session_settings"]')!;
    expect(settings).not.toBeNull();
    // O campo não divide a linha com os botões.
    expect(field.parentElement!.contains(settings)).toBe(false);
    expect(container.querySelector<HTMLButtonElement>('[aria-label="composer_gravar_audio"]')!.title).toBe('composer_mic_style_hint');
    expect(container.querySelector('[aria-label="composer_adicionar_ao_chat"]')).not.toBeNull();
    // Comandos e estilo do ditado moram no +, não na linha.
    expect(container.querySelector('[aria-label="comandos_titulo"]')).toBeNull();
    act(() => root.unmount());
  });

  it('trabalhando com texto sem terminal: mostra Orientar, microfone, chip e Enviar', async () => {
    composerChat.state = 'working';
    const { container, root } = await render(createElement(Composer, { serverId: 's1', name: 'sess', draft: 'texto', headless: true, onStop: () => {}, queuePanel: noQueue }));
    expect(container.querySelector('[aria-label="composer_steer_current_text"]')).not.toBeNull();
    expect(container.querySelector('[aria-label="composer_gravar_audio"]')).not.toBeNull();
    expect(container.querySelector('[aria-label="composer_session_settings"]')).not.toBeNull();
    expect(container.querySelector('[aria-label="composer_enviar_mensagem"]')).not.toBeNull();
    act(() => root.unmount());
  });
});

describe('primeiro texto recuperado no Composer', () => {
  const props = { serverId: 's1', name: 'sess', draft: 'primeiro texto', firstInputId: 'attempt-1', queuePanel: noQueue };
  beforeEach(() => {
    composerChat.send.mockClear(); firstInput.confirm.mockClear(); firstInput.send.mockReset();
    firstInput.attempt = {
      id: 'attempt-1', serverId: 's1', body: { name: 'sess', cwd: '/repo', provider: 'codex' },
      sessionName: 'sess', text: 'primeiro texto', phase: 'created',
    };
  });

  it('montar/remontar não envia; dois toques explícitos fazem um envio à mesma sessão sem eco local', async () => {
    let done!: () => void;
    firstInput.send.mockImplementation(() => new Promise<void>((resolve) => { done = resolve; }));
    const first = await render(createElement(Composer, props));
    act(() => first.root.unmount());
    const { container, root } = await render(createElement(Composer, props));
    expect(container.querySelector('textarea')!.value).toBe('primeiro texto');
    expect(firstInput.send).not.toHaveBeenCalled();
    const send = container.querySelector<HTMLButtonElement>('[aria-label="composer_enviar_mensagem"]')!;
    act(() => { send.click(); send.click(); });
    expect(firstInput.send).toHaveBeenCalledExactlyOnceWith('s1', 'attempt-1');
    expect(composerChat.send).not.toHaveBeenCalled();
    firstInput.attempt!.phase = 'sent';
    await act(async () => done());
    expect(firstInput.confirm).toHaveBeenCalledWith('attempt-1');
    act(() => root.unmount());
  });

  it('recusa conserva rascunho; resultado incerto não repete POST', async () => {
    firstInput.send.mockResolvedValue(undefined);
    const { container, root } = await render(createElement(Composer, props));
    await act(async () => container.querySelector<HTMLButtonElement>('[aria-label="composer_enviar_mensagem"]')!.click());
    expect(container.querySelector('textarea')!.value).toBe('primeiro texto');
    expect(firstInput.confirm).not.toHaveBeenCalled();
    firstInput.attempt!.phase = 'send_unknown';
    firstInput.send.mockClear();
    await act(async () => container.querySelector<HTMLButtonElement>('[aria-label="composer_enviar_mensagem"]')!.click());
    expect(firstInput.send).not.toHaveBeenCalled();
    expect(composerChat.send).not.toHaveBeenCalled();
    expect(container.querySelector('textarea')!.value).toBe('primeiro texto');
    expect(container.textContent).toContain('nova_conversa_envio_incerto');
    act(() => root.unmount());
  });

  it('ACK confirmado antes de o botão atualizar não repete o texto que ainda aparece', async () => {
    const { container, root } = await render(createElement(Composer, props));
    firstInput.attempt = null;
    await act(async () => container.querySelector<HTMLButtonElement>('[aria-label="composer_enviar_mensagem"]')!.click());
    expect(firstInput.send).not.toHaveBeenCalled();
    expect(composerChat.send).not.toHaveBeenCalled();
    expect(container.querySelector('textarea')!.value).toBe('');
    act(() => root.unmount());
  });

  it('ACK tardio preserva nova edição e sent não repete o primeiro input', async () => {
    let done!: () => void;
    firstInput.send.mockImplementation(() => new Promise<void>((resolve) => { done = resolve; }));
    const { container, root } = await render(createElement(Composer, props));
    act(() => container.querySelector<HTMLButtonElement>('[aria-label="composer_enviar_mensagem"]')!.click());
    const field = container.querySelector('textarea')!;
    act(() => {
      Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value')!.set!.call(field, 'texto novo');
      field.dispatchEvent(new Event('input', { bubbles: true }));
    });
    firstInput.attempt!.phase = 'sent';
    await act(async () => done());
    expect(field.value).toBe('texto novo');
    expect(firstInput.confirm).toHaveBeenCalledWith('attempt-1');
    act(() => root.unmount());
    const reopened = await render(createElement(Composer, props));
    // A edição guardada vence o texto de handoff que a rota ainda carrega; enviá-la é mensagem nova.
    expect(reopened.container.querySelector('textarea')!.value).toBe('texto novo');
    await act(async () => reopened.container.querySelector<HTMLButtonElement>('[aria-label="composer_enviar_mensagem"]')!.click());
    expect(firstInput.send).toHaveBeenCalledTimes(1);
    expect(composerChat.send).toHaveBeenCalledExactlyOnceWith('texto novo', expect.any(Number));
    act(() => reopened.root.unmount());
  });
});

describe('primeiro envio incerto com os stores reais', () => {
  const props = { serverId: 's1', name: 'recuperada', draft: 'primeiro texto', firstInputId: 'unknown-real', queuePanel: noQueue };
  afterEach(() => { vi.useRealTimers(); _resetNewConversationForTests(); });
  beforeEach(() => {
    realFirstInput.enabled = true;
    realFirstInput.send.mockReset().mockResolvedValue(undefined);
    realFirstInput.history.mockReset().mockResolvedValue([]);
    _resetNewConversationForTests();
    storage.memory.set('create.attempt.v1:s1', JSON.stringify({
      id: 'unknown-real', serverId: 's1', body: { name: 'recuperada', cwd: '/repo', provider: 'codex' },
      sessionName: 'recuperada', text: 'primeiro texto', phase: 'send_unknown',
    }));
    storage.memory.set('draft.v1:s1::recuperada', JSON.stringify({
      version: 1, text: 'primeiro texto', revision: 1, transcript: null, attachment: null,
      submission: { text: 'primeiro texto', draftRevision: 1, status: 'unknown' },
    }));
    restoreAttempt('s1');
  });

  it('Recuperar não envia nem descarta; Enviar uma vez remove tentativa e libera Nova conversa', async () => {
    let done!: () => void;
    realFirstInput.send.mockImplementationOnce(() => new Promise<void>((resolve) => { done = resolve; }));
    const { container, root } = await render(createElement(Composer, props));
    const send = container.querySelector<HTMLButtonElement>('[aria-label="composer_enviar_mensagem"]')!;
    await act(async () => send.click());
    expect(realFirstInput.send).not.toHaveBeenCalled();
    expect(container.textContent).toContain('nova_conversa_envio_incerto');
    act(() => button(container, 'composer_draft_recover')!.click());
    expect(realFirstInput.send).not.toHaveBeenCalled();
    expect(useNewConversation.getState().attempts.s1?.phase).toBe('send_unknown');
    expect(send.disabled).toBe(false);
    act(() => { send.click(); send.click(); });
    expect(realFirstInput.send).toHaveBeenCalledExactlyOnceWith({ id: 's1' }, 'recuperada', 'primeiro texto');
    expect(useNewConversation.getState().attempts.s1).toBeUndefined();
    expect(storage.memory.has('create.attempt.v1:s1')).toBe(false);
    await act(async () => done());
    expect(container.querySelector('textarea')!.value).toBe('');
    act(() => root.unmount());

    const creation = await render(createElement(NewConversation,
      newConversationProps({ id: 's1', label: 'S1', baseUrl: 'http://s1', token: 'fixture' })));
    act(() => {
      const field = creation.container.querySelector('textarea')!;
      Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value')!.set!.call(field, 'outra conversa');
      field.dispatchEvent(new Event('input', { bubbles: true }));
    });
    expect((button(creation.container, 'nova_conversa_enviar') as HTMLButtonElement).disabled).toBe(false);
    expect(realFirstInput.send).toHaveBeenCalledTimes(1);
    act(() => creation.root.unmount());
  });

  it('created que fica incerto no próprio toque não repete o POST nem abandona a tentativa', async () => {
    storage.memory.set('create.attempt.v1:s1', JSON.stringify({
      id: props.firstInputId, serverId: 's1', body: { name: props.name, cwd: '/repo', provider: 'codex' },
      sessionName: props.name, text: props.draft, phase: 'created',
    }));
    storage.memory.delete('draft.v1:s1::recuperada');
    _resetNewConversationForTests();
    restoreAttempt('s1');
    realFirstInput.send.mockRejectedValueOnce(new TypeError('offline'));
    const { container, root } = await render(createElement(Composer, props));
    await act(async () => container.querySelector<HTMLButtonElement>('[aria-label="composer_enviar_mensagem"]')!.click());
    expect(realFirstInput.send).toHaveBeenCalledExactlyOnceWith({ id: 's1' }, props.name, props.draft);
    expect(useNewConversation.getState().attempts.s1?.phase).toBe('send_unknown');
    expect(container.textContent).toContain('nova_conversa_envio_incerto');
    expect(container.querySelector('textarea')!.value).toBe(props.draft);
    act(() => root.unmount());
  });

  it('envio automático após Recuperar não abandona send_unknown nem dispara POST', async () => {
    vi.useFakeTimers();
    realFirstInput.transcribe.mockResolvedValueOnce({ text: 'Execute a tarefa solicitada.', raw: '', aviso: null });
    const { container, root } = await render(createElement(Composer, props));
    act(() => button(container, 'composer_draft_recover')!.click());
    act(() => {
      const field = container.querySelector('textarea')!;
      Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value')!.set!.call(field, '');
      field.dispatchEvent(new Event('input', { bubbles: true }));
    });
    await act(async () => container.querySelector<HTMLButtonElement>('[aria-label="composer_gravar_audio"]')!.click());
    await act(async () => voiceInput.onFim!(new File(['audio'], 'voice.m4a', { type: 'audio/mp4' }), 'silencio', 'file:///voice.m4a'));
    expect(container.textContent).toContain('composer_enviando_cancelar');
    await act(async () => vi.advanceTimersByTimeAsync(3250));
    expect(realFirstInput.send).not.toHaveBeenCalled();
    expect(useNewConversation.getState().attempts.s1?.phase).toBe('send_unknown');
    expect(storage.memory.has('create.attempt.v1:s1')).toBe(true);
    expect(container.textContent).toContain('nova_conversa_envio_incerto');
    act(() => root.unmount());
  });

  it.each(['inFlight', 'remove'])('abandono impedido por %s conserva tentativa e texto sem POST', async (failure) => {
    const { container, root } = await render(createElement(Composer, props));
    act(() => button(container, 'composer_draft_recover')!.click());
    let finish!: (events: []) => void;
    realFirstInput.history.mockImplementationOnce(() => new Promise<[]>((resolve) => { finish = resolve; }));
    const recovery = failure === 'inFlight' ? recoverAttempt('s1') : null;
    storage.failRemove = failure === 'remove';
    await act(async () => container.querySelector<HTMLButtonElement>('[aria-label="composer_enviar_mensagem"]')!.click());
    expect(realFirstInput.send).not.toHaveBeenCalled();
    expect(useNewConversation.getState().attempts.s1?.phase).toBe('send_unknown');
    expect(storage.memory.has('create.attempt.v1:s1')).toBe(true);
    expect(container.querySelector('textarea')!.value).toBe(props.draft);
    expect(container.textContent).toContain(failure === 'remove' ? 'nova_conversa_salvar_erro' : 'nova_conversa_envio_incerto');
    if (recovery) await act(async () => { finish([]); await recovery; });
    act(() => root.unmount());
  });

  it('ACK da nova ação preserva edição posterior e o destino original', async () => {
    let done!: () => void;
    realFirstInput.send.mockImplementationOnce(() => new Promise<void>((resolve) => { done = resolve; }));
    const { container, root } = await render(createElement(Composer, props));
    act(() => button(container, 'composer_draft_recover')!.click());
    act(() => container.querySelector<HTMLButtonElement>('[aria-label="composer_enviar_mensagem"]')!.click());
    act(() => {
      const field = container.querySelector('textarea')!;
      Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value')!.set!.call(field, 'edição durante envio');
      field.dispatchEvent(new Event('input', { bubbles: true }));
    });
    await act(async () => done());
    expect(container.querySelector('textarea')!.value).toBe('edição durante envio');
    expect(JSON.parse(storage.memory.get('draft.v1:s1::recuperada')!).text).toBe('edição durante envio');
    expect(realFirstInput.send).toHaveBeenCalledExactlyOnceWith({ id: 's1' }, 'recuperada', 'primeiro texto');
    expect(storage.memory.has('draft.v1:s2::recuperada')).toBe(false);
    act(() => root.unmount());
  });
});

describe('rascunho guardado no Composer', () => {
  const props = { serverId: 's1', name: 'sess', queuePanel: noQueue };
  const type = (container: HTMLElement, value: string) => act(() => {
    const field = container.querySelector('textarea')!;
    Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value')!.set!.call(field, value);
    field.dispatchEvent(new Event('input', { bubbles: true }));
  });
  const stored = (serverId: string, name: string) => {
    const raw = storage.memory.get(`draft.v1:${serverId}::${name}`);
    return raw ? JSON.parse(raw) as { text: string; revision: number; transcript: string | null } : null;
  };

  it.each(['sending', 'unknown', 'rejected'])('reabre upload confirmado com input %s sem reenviar; Recuperar devolve só o texto', async (status) => {
    composerChat.send.mockClear();
    const attachment = {
      uri: 'file:///draft-attachments/1-1.txt', name: 'nota.txt', mime: 'text/plain', kind: 'file',
      uploadedPath: '/uploads/nota.txt', uploadedFor: { serverId: 's1', name: 'sess', transcript: null },
    };
    storage.memory.set('draft.v1:s1::sess', JSON.stringify({
      version: 1, text: 'nota', revision: 1, transcript: null, attachment,
      submission: { text: 'nota — 📎 board_arquivo: /uploads/nota.txt', draftRevision: 1, status },
    }));
    const first = await render(createElement(Composer, props));
    act(() => first.root.unmount());
    const reopened = await render(createElement(Composer, props));
    expect(composerChat.send).not.toHaveBeenCalled();
    expect(reopened.container.textContent).toContain('nota.txt');
    act(() => button(reopened.container, 'composer_draft_recover')!.click());
    expect(reopened.container.querySelector('textarea')!.value).toBe('nota');
    expect(JSON.parse(storage.memory.get('draft.v1:s1::sess')!)).toMatchObject({ attachment, submission: null });
    act(() => reopened.root.unmount());
  });

  it('recuperação não troca anexo atual; depois de removê-lo adota o anterior sem reutilizar upload da sessão morta', async () => {
    const attachment = { uri: 'file:///draft-attachments/2-1.txt', name: 'atual.txt', mime: 'text/plain', kind: 'file' };
    const previous = { ...attachment, uri: 'file:///draft-attachments/1-1.txt', name: 'anterior.txt',
      uploadedPath: '/uploads/anterior.txt', uploadedFor: { serverId: 's1', name: 'sess', transcript: '/old' } };
    storage.memory.set('draft.v1:s1::sess', JSON.stringify({
      version: 1, text: 'atual', revision: 1, transcript: null, attachment, submission: null,
    }));
    storage.memory.set('draft.v1.recoverable:s1::sess', JSON.stringify({
      version: 1, text: 'anterior', revision: 1, transcript: '/old', attachment: previous, submission: null,
    }));
    const { container, root } = await render(createElement(Composer, props));
    act(() => button(container, 'composer_draft_recover')!.click());
    expect(container.textContent).toContain('composer_draft_recover_attach_busy');
    expect(container.querySelector('textarea')!.value).toBe('atual');
    expect(JSON.parse(storage.memory.get('draft.v1:s1::sess')!).attachment).toEqual(attachment);
    expect(storage.memory.has('draft.v1.recoverable:s1::sess')).toBe(true);
    act(() => container.querySelector<HTMLButtonElement>('[aria-label="board_remover_anexo"]')!.click());
    act(() => button(container, 'composer_draft_recover')!.click());
    expect(container.querySelector('textarea')!.value).toBe('atual\nanterior');
    expect(JSON.parse(storage.memory.get('draft.v1:s1::sess')!).attachment).toEqual({
      uri: previous.uri, name: previous.name, mime: previous.mime, kind: previous.kind,
    });
    expect(storage.memory.has('draft.v1.recoverable:s1::sess')).toBe(false);
    act(() => root.unmount());
  });

  it('cada edição grava na conversa de origem; desmontar não apaga e outro servidor com mesmo nome não recebe', async () => {
    sessionsState.rows = [{ serverId: 's1', name: 'sess', jsonl: '/t/a.jsonl' }];
    const first = await render(createElement(Composer, props));
    type(first.container, 'o');
    type(first.container, 'oi');
    expect(stored('s1', 'sess')).toMatchObject({ text: 'oi', revision: 2, transcript: '/t/a.jsonl' });
    act(() => first.root.unmount());
    expect(stored('s1', 'sess')?.text).toBe('oi');

    const other = await render(createElement(Composer, { serverId: 's2', name: 'sess', queuePanel: noQueue }));
    expect(other.container.querySelector('textarea')!.value).toBe('');
    act(() => other.root.unmount());
    const reopened = await render(createElement(Composer, props));
    expect(reopened.container.querySelector('textarea')!.value).toBe('oi');
    act(() => reopened.root.unmount());
  });

  it('texto devolvido pelo cancelar das opções é adotado e guardado na mesma conversa', async () => {
    const { container, root } = await render(createElement(Composer, props));
    await act(async () => root.render(createElement(Composer, { ...props, draft: 'resposta cancelada' })));
    expect(container.querySelector('textarea')!.value).toBe('resposta cancelada');
    expect(stored('s1', 'sess')?.text).toBe('resposta cancelada');
    act(() => root.unmount());
  });

  it('texto devolvido pelo Parar entra antes do digitado, e o mesmo texto devolvido de novo entra de novo', async () => {
    const adopted = vi.fn();
    const { container, root } = await render(createElement(Composer, { ...props, onReturnedAdopted: adopted }));
    type(container, 'digitado');
    await act(async () => root.render(createElement(Composer, { ...props, returned: { text: 'pendente' }, onReturnedAdopted: adopted })));
    expect(container.querySelector('textarea')!.value).toBe('pendente\n\ndigitado');
    await act(async () => root.render(createElement(Composer, { ...props, returned: { text: 'pendente' }, onReturnedAdopted: adopted })));
    expect(container.querySelector('textarea')!.value).toBe('pendente\n\npendente\n\ndigitado');
    expect(adopted).toHaveBeenCalledTimes(2);
    expect(stored('s1', 'sess')?.text).toBe('pendente\n\npendente\n\ndigitado');
    act(() => root.unmount());
  });

  it('primeiro jsonl associa o rascunho provisório sem tratar como sessão recriada', async () => {
    sessionsState.rows = [{ serverId: 's1', name: 'sess', jsonl: null }];
    const { container, root } = await render(createElement(Composer, props));
    type(container, 'antes do transcript');
    expect(stored('s1', 'sess')?.transcript).toBeNull();
    sessionsState.rows = [{ serverId: 's1', name: 'sess', jsonl: '/t/first.jsonl' }];
    await act(async () => root.render(createElement(Composer, props)));
    expect(container.querySelector('textarea')!.value).toBe('antes do transcript');
    expect(stored('s1', 'sess')?.transcript).toBe('/t/first.jsonl');
    expect(container.textContent).not.toContain('composer_draft_previous');
    act(() => root.unmount());
  });

  it('sessão recriada não recebe o texto antigo; ele sobrevive a texto novo e reabertura até recuperar', async () => {
    sessionsState.rows = [{ serverId: 's1', name: 'sess', jsonl: '/t/old.jsonl' }];
    const old = await render(createElement(Composer, props));
    type(old.container, 'texto da sessão morta');
    act(() => old.root.unmount());

    sessionsState.rows = [{ serverId: 's1', name: 'sess', jsonl: '/t/new.jsonl' }];
    const recreated = await render(createElement(Composer, props));
    expect(recreated.container.querySelector('textarea')!.value).toBe('');
    expect(recreated.container.textContent).toContain('composer_draft_previous');
    type(recreated.container, 'texto novo');
    expect(stored('s1', 'sess')).toMatchObject({ text: 'texto novo', transcript: '/t/new.jsonl' });
    act(() => recreated.root.unmount());

    const reopened = await render(createElement(Composer, props));
    expect(reopened.container.querySelector('textarea')!.value).toBe('texto novo');
    act(() => button(reopened.container, 'composer_draft_recover')!.click());
    expect(reopened.container.querySelector('textarea')!.value).toBe('texto novo\ntexto da sessão morta');
    expect(stored('s1', 'sess')?.text).toBe('texto novo\ntexto da sessão morta');
    expect(storage.memory.has('draft.v1.recoverable:s1::sess')).toBe(false);
    expect(reopened.container.textContent).not.toContain('composer_draft_previous');
    act(() => reopened.root.unmount());
  });

  it('sessão recriada enquanto aberta tira o texto antigo do campo e oferece recuperar; descartar não reoferece', async () => {
    sessionsState.rows = [{ serverId: 's1', name: 'sess', jsonl: '/t/old.jsonl' }];
    const { container, root } = await render(createElement(Composer, props));
    type(container, 'antigo');
    sessionsState.rows = [{ serverId: 's1', name: 'sess', jsonl: '/t/new.jsonl' }];
    await act(async () => root.render(createElement(Composer, props)));
    expect(container.querySelector('textarea')!.value).toBe('');
    expect(container.textContent).toContain('composer_draft_previous');
    act(() => button(container, 'composer_draft_discard')!.click());
    expect(container.textContent).not.toContain('composer_draft_previous');
    act(() => root.unmount());
    const reopened = await render(createElement(Composer, props));
    expect(reopened.container.textContent).not.toContain('composer_draft_previous');
    act(() => reopened.root.unmount());
  });

  it('"Ler de novo" após falha na recriação não traz o texto morto ao campo e Recuperar o devolve uma vez', async () => {
    sessionsState.rows = [{ serverId: 's1', name: 'sess', jsonl: '/t/old.jsonl' }];
    const { container, root } = await render(createElement(Composer, props));
    type(container, 'hello');
    storage.failSet = true;
    sessionsState.rows = [{ serverId: 's1', name: 'sess', jsonl: '/t/new.jsonl' }];
    await act(async () => root.render(createElement(Composer, props)));
    expect(container.textContent).toContain('draft_write_error');
    storage.failSet = false;
    act(() => button(container, 'composer_draft_read_again')!.click());
    expect(container.querySelector('textarea')!.value).toBe('');
    expect(container.textContent).toContain('composer_draft_previous');
    expect(stored('s1', 'sess')?.text).not.toBe('hello');
    act(() => button(container, 'composer_draft_recover')!.click());
    expect(container.querySelector('textarea')!.value).toBe('hello');
    act(() => root.unmount());
  });

  it('falha ao gravar aparece como aviso e o texto continua no campo', async () => {
    const { container, root } = await render(createElement(Composer, props));
    storage.failSet = true;
    type(container, 'sem espaço');
    expect(container.querySelector('textarea')!.value).toBe('sem espaço');
    expect(container.textContent).toContain('draft_write_error');
    act(() => root.unmount());
  });

  it('rascunho em formato inválido avisa sem travar e a edição seguinte grava por cima', async () => {
    storage.memory.set('draft.v1:s1::sess', '{"text":');
    const { container, root } = await render(createElement(Composer, props));
    expect(container.textContent).toContain('draft_invalid');
    type(container, 'recomeço');
    expect(stored('s1', 'sess')?.text).toBe('recomeço');
    act(() => root.unmount());
  });
});

describe('Orientar texto do composer com os stores reais', () => {
  const props = { serverId: 's1', name: 'sess', sessionProvider: 'codex', queuePanel: noQueue };
  const steer = (c: HTMLElement) => c.querySelector<HTMLButtonElement>('[aria-label="composer_steer_current_text"]');
  const send = (c: HTMLElement) => c.querySelector<HTMLButtonElement>('[aria-label="composer_enviar_mensagem"]')!;
  const type = (container: HTMLElement, value: string) => act(() => {
    const field = container.querySelector('textarea')!;
    Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value')!.set!.call(field, value);
    field.dispatchEvent(new Event('input', { bubbles: true }));
  });
  beforeEach(() => {
    _resetChatsForTests();
    realFirstInput.enabled = true;
    realFirstInput.steer.mockReset().mockResolvedValue({ ok: true });
    realFirstInput.send.mockReset().mockResolvedValue(undefined);
    useServers.getState().activeId = 's2';
    voiceInput.recording = false;
    firstInput.attempt = null;
    chatStore('s1', 'sess').use.setState({ stateEvent: { session: 'sess', state: 'working' } });
  });
  afterEach(() => {
    voiceInput.recording = false;
    setDictationInFlight('s1', 'sess', 'steer-voice', false);
    _resetChatsForTests();
  });

  it.each([['codex', false], ['codex', true], ['claude', true]] as const)(
    '%s sem terminal=%s: texto literal vai para A com B ativo, eco único e limpeza só após ACK', async (sessionProvider, headless) => {
      let done!: (result: { ok: boolean }) => void;
      realFirstInput.steer.mockImplementationOnce(() => new Promise((resolve) => { done = resolve; }));
      const chat = chatStore('s1', 'sess');
      chat.use.setState({ events: [{ kind: 'user_msg', id: 'queued-previous', text: 'outra na fila' }] });
      const { container, root } = await render(createElement(Composer, { ...props, sessionProvider, headless }));
      const literal = '  muda só esta linha\nsem /clear  ';
      type(container, literal);
      const orient = steer(container)!;
      expect(orient.disabled).toBe(false);
      expect(orient.title).toBe('codex_orientar_ajuda');
      expect(useServers.getState().activeId).toBe('s2');
      const source = useServers.getState().servers.find((s) => s.id === 's1')!;
      act(() => { orient.click(); orient.click(); send(container).click(); });
      expect(realFirstInput.steer).toHaveBeenCalledExactlyOnceWith('sess', literal, source);
      expect(realFirstInput.steer.mock.calls[0][2]).toBe(source);
      expect(realFirstInput.send).not.toHaveBeenCalled();
      expect(orient.disabled).toBe(true);
      expect(send(container).disabled).toBe(true);
      expect(container.querySelector('textarea')!.value).toBe(literal);
      expect(readDraft('s1', 'sess')).toMatchObject({ text: literal, submission: { text: literal } });
      expect(chat.use.getState().pending).toEqual([{ id: expect.any(String), text: literal, solid: false }]);
      // O resultado de texto não promove nem apaga as mensagens que já estavam na fila.
      await act(async () => done({ ok: true, promoted: true } as { ok: boolean }));
      expect(container.querySelector('textarea')!.value).toBe('');
      expect(readDraft('s1', 'sess')).toMatchObject({ text: '', submission: null });
      expect(chat.use.getState().events[0].id).toBe('queued-previous');
      expect(chat.use.getState().pending).toHaveLength(1);
      act(() => root.unmount());
    },
  );

  it.each(['novo texto', 'original'])('ACK preserva uma edição feita no pedido, mesmo voltando para %s', async (next) => {
    let done!: (result: { ok: boolean }) => void;
    realFirstInput.steer.mockImplementationOnce(() => new Promise((resolve) => { done = resolve; }));
    const { container, root } = await render(createElement(Composer, props));
    type(container, 'original');
    act(() => steer(container)!.click());
    type(container, 'edição intermediária');
    type(container, next);
    await act(async () => done({ ok: true }));
    expect(container.querySelector('textarea')!.value).toBe(next);
    expect(readDraft('s1', 'sess')).toMatchObject({ text: next, submission: null });
    act(() => root.unmount());
  });

  it.each([false, true])('falha visível conserva o rascunho e edição posterior=%s', async (edited) => {
    let fail!: (error: Error) => void;
    realFirstInput.steer.mockImplementationOnce(() => new Promise((_, reject) => { fail = reject; }));
    const { container, root } = await render(createElement(Composer, props));
    type(container, 'não perder');
    act(() => steer(container)!.click());
    if (edited) type(container, 'edição durante pedido');
    await act(async () => fail(Object.assign(new Error('texto recusado'), { status: 422 })));
    const expected = edited ? 'edição durante pedido' : 'não perder';
    expect(container.textContent).toContain('texto recusado');
    expect(container.querySelector('textarea')!.value).toBe(expected);
    expect(readDraft('s1', 'sess')).toMatchObject({ text: expected, submission: { status: 'rejected', text: 'não perder' } });
    expect(chatStore('s1', 'sess').use.getState().pending).toEqual([]);
    expect(realFirstInput.send).not.toHaveBeenCalled();
    act(() => root.unmount());
  });

  it.each(['rede', '409'])('resultado incerto (%s) conserva texto e eco sem fingir recusa', async (cause) => {
    realFirstInput.steer.mockRejectedValueOnce(cause === 'rede' ? new TypeError('offline')
      : Object.assign(new Error('operação escrita sem confirmação'), { status: 409 }));
    const { container, root } = await render(createElement(Composer, props));
    type(container, 'conservar');
    await act(async () => steer(container)!.click());
    expect(container.textContent).toContain(cause === 'rede' ? 'chat_envio_incerto' : 'queue_steer_uncertain');
    expect(container.querySelector('textarea')!.value).toBe('conservar');
    expect(readDraft('s1', 'sess')).toMatchObject({ text: 'conservar', submission: { status: 'unknown' } });
    expect(chatStore('s1', 'sess').use.getState().pending).toHaveLength(1);
    act(() => root.unmount());
  });

  it('Enviar em voo também bloqueia Orientar no mesmo toque e até terminar', async () => {
    let done!: () => void;
    realFirstInput.send.mockImplementationOnce(() => new Promise<void>((resolve) => { done = resolve; }));
    const { container, root } = await render(createElement(Composer, props));
    type(container, 'mensagem');
    act(() => { send(container).click(); steer(container)!.click(); });
    expect(realFirstInput.send).toHaveBeenCalledTimes(1);
    expect(realFirstInput.steer).not.toHaveBeenCalled();
    expect(steer(container)!.disabled).toBe(true);
    await act(async () => done());
    act(() => root.unmount());
  });

  it.each([
    ['codex', false, 'idle', 'texto'], ['codex', true, 'awaiting_input', 'texto'],
    ['claude', false, 'working', 'texto'], ['kimi', false, 'working', 'texto'],
    ['pi', true, 'working', 'texto'], ['omp', true, 'working', 'texto'],
    ['codex', false, 'working', '   '], ['codex', false, 'working', '  /model gpt'],
  ] as const)('%s sem terminal=%s estado=%s texto=%s não oferece Orientar', async (sessionProvider, headless, state, draft) => {
    chatStore('s1', 'sess').use.setState({ stateEvent: { session: 'sess', state } });
    const { container, root } = await render(createElement(Composer, { ...props, sessionProvider, headless, draft }));
    expect(steer(container)).toBeNull();
    expect(realFirstInput.steer).not.toHaveBeenCalled();
    act(() => root.unmount());
  });

  it.each(['anexo', 'edição da fila'])('bloqueia Orientar com %s sem perder texto', async (blocked) => {
    const panel = blocked === 'edição da fila'
      ? { entries: [{ id: 'q1', text: 'mensagem da fila', ts: null }], actions: true } : noQueue;
    writeDraft('s1', 'sess', { version: 1, text: 'rascunho', revision: 1, transcript: null, submission: null,
      attachment: blocked === 'anexo' ? { uri: 'file:///foto.png', name: 'foto.png', mime: 'image/png', kind: 'image' } : null });
    if (blocked === 'edição da fila') {
      const chat = chatStore('s1', 'sess');
      chat.use.setState({ queuePanel: panel });
      chat.startQueueEdit('q1', 'rascunho');
    }
    const { container, root } = await render(createElement(Composer, { ...props, queuePanel: panel }));
    const orient = steer(container);
    expect(!orient || orient.disabled).toBe(true);
    act(() => orient?.click());
    expect(realFirstInput.steer).not.toHaveBeenCalled();
    expect(readDraft('s1', 'sess')?.text).toBe('rascunho');
    act(() => root.unmount());
  });

  it('mandar para o grupo esconde Orientar, desligar devolve sem mudar o texto', async () => {
    sessionsState.rows = [{ serverId: 's1', name: 'sess', pair_peers: ['par'] }];
    const { container, root } = await render(createElement(Composer, props));
    type(container, 'só esta conversa');
    expect(steer(container)).not.toBeNull();
    act(() => button(container, 'composer_adicionar_ao_chat')!.click());
    act(() => button(container, 'send-to-group')!.click());
    expect(steer(container)).toBeNull();
    expect(container.querySelector('textarea')!.value).toBe('só esta conversa');
    act(() => button(container, 'composer_mandar_grupo')!.click());
    expect(steer(container)!.disabled).toBe(false);
    expect(realFirstInput.steer).not.toHaveBeenCalled();
    act(() => root.unmount());
  });

  it('Kimi continua promovendo a fila inteira com undefined, sem consumir o rascunho', async () => {
    realFirstInput.steer.mockResolvedValueOnce({ ok: true, promoted: true });
    const chat = chatStore('s1', 'sess');
    chat.use.setState({ events: [{ kind: 'user_msg', id: 'queued-kimi', text: 'na fila' }] });
    const { container, root } = await render(createElement(Composer, { ...props, sessionProvider: 'kimi', draft: 'ainda no campo' }));
    expect(steer(container)).toBeNull();
    await act(async () => button(container, 'composer_fila_aria')!.click());
    expect(realFirstInput.steer).toHaveBeenCalledExactlyOnceWith('sess', undefined, useServers.getState().servers.find((s) => s.id === 's1'));
    expect(chat.use.getState().events).toEqual([]);
    expect(chat.use.getState().pending).toEqual([]);
    expect(readDraft('s1', 'sess')?.text).toBe('ainda no campo');
    expect(container.querySelector('textarea')!.value).toBe('ainda no campo');
    act(() => root.unmount());
  });
});

describe('Enviar encerra o ditado e aguarda o texto completo', () => {
  const props = { serverId: 's1', name: 'sess', sessionProvider: 'codex', queuePanel: noQueue, onStop: () => {} };
  const roots = new Set<ReturnType<typeof createRoot>>();
  const send = (c: HTMLElement) => c.querySelector<HTMLButtonElement>('[aria-label="composer_enviar_mensagem"]')!;
  const steer = (c: HTMLElement) => c.querySelector<HTMLButtonElement>('[aria-label="composer_steer_current_text"]')!;
  const type = (container: HTMLElement, value: string) => act(() => {
    const field = container.querySelector('textarea')!;
    Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value')!.set!.call(field, value);
    field.dispatchEvent(new Event('input', { bubbles: true }));
  });
  const answer = { path: '/up/sess/ditado.m4a', text: 'texto organizado', raw: 'texto cru', aviso: null as string | null };
  const pendingTranscription = () => {
    let done!: (value: typeof answer) => void;
    let fail!: (e: Error) => void;
    realFirstInput.transcribe.mockImplementationOnce(() => new Promise((resolve, reject) => { done = resolve; fail = reject; }));
    return { done: (value = answer) => done(value), fail: (e: Error) => fail(e) };
  };
  const mount = async (before = '') => {
    const mounted = await render(createElement(Composer, props));
    roots.add(mounted.root);
    if (before) type(mounted.container, before);
    return mounted;
  };
  const start = async (before = '') => {
    const mounted = await mount(before);
    await act(async () => button(mounted.container, 'composer_gravar_audio')!.click());
    expect(button(mounted.container, 'composer_parar_gravacao')).toBeDefined();
    return mounted;
  };
  const close = (root: ReturnType<typeof createRoot>) => act(() => { root.unmount(); roots.delete(root); });
  const complete = async (request: ReturnType<typeof pendingTranscription>, value = answer) => {
    await act(async () => { request.done(value); await voiceInput.finished; });
  };
  beforeEach(() => {
    vi.useFakeTimers();
    _resetChatsForTests();
    realFirstInput.enabled = true;
    realFirstInput.send.mockReset().mockResolvedValue(undefined);
    realFirstInput.steer.mockReset().mockResolvedValue({ ok: true });
    realFirstInput.upload.mockReset().mockResolvedValue({ path: answer.path });
    realFirstInput.transcribe.mockReset().mockResolvedValue(answer);
    voiceInput.simulateRecorder = true;
    voiceInput.recording = false;
    voiceInput.delayStop = false;
    voiceInput.stopPending = null;
    voiceInput.finishStop = null;
    voiceInput.finished = null;
    voiceInput.stop.mockReset();
    appLifecycle.currentState = 'active';
    focusNavigation.focused = true;
    firstInput.attempt = null;
    useServers.getState().activeId = 's2';
    sessionsState.rows = [{ serverId: 's1', name: 'sess', jsonl: '/t/voice.jsonl' }];
    chatStore('s1', 'sess').use.setState({ stateEvent: { session: 'sess', state: 'working' } });
  });
  afterEach(() => {
    act(() => { for (const root of roots) root.unmount(); roots.clear(); });
    voiceInput.simulateRecorder = false;
    voiceInput.delayStop = false;
    voiceInput.stopPending = null;
    voiceInput.finishStop = null;
    focusNavigation.focused = true;
    appLifecycle.currentState = 'active';
    _resetChatsForTests();
    vi.useRealTimers();
  });

  it.each(['', 'rascunho anterior'])('gravando com campo "%s": Enviar aparece e espera upload e organização', async (before) => {
    let uploaded!: (result: { path: string }) => void;
    realFirstInput.upload.mockImplementationOnce(() => new Promise((resolve) => { uploaded = resolve; }));
    const request = pendingTranscription();
    const { container } = await start(before);
    expect(send(container)).not.toBeNull();
    expect(send(container).disabled).toBe(false);
    expect(button(container, 'composer_parar')).toBeUndefined();
    const source = useServers.getState().servers.find((s) => s.id === 's1')!;
    await act(async () => { send(container).click(); send(container).click(); });
    expect(voiceInput.stop).toHaveBeenCalledExactlyOnceWith('botao');
    expect(realFirstInput.upload).toHaveBeenCalledExactlyOnceWith(source, 'sess', expect.any(File), { audioOnly: true });
    expect(realFirstInput.transcribe).not.toHaveBeenCalled();
    expect(realFirstInput.send).not.toHaveBeenCalled();
    expect(readDraft('s1', 'sess')?.text).toBe(before);
    await act(async () => uploaded({ path: answer.path }));
    expect(realFirstInput.transcribe).toHaveBeenCalledExactlyOnceWith(source, 'sess', 'ditado.m4a', { limpar: true, estilo: undefined });
    expect(realFirstInput.send).not.toHaveBeenCalled();
    expect(send(container).disabled).toBe(true);
    await complete(request);
    const full = before ? `${before} ${answer.text}` : answer.text;
    expect(realFirstInput.send).toHaveBeenCalledExactlyOnceWith(source, 'sess', full);
    expect(chatStore('s1', 'sess').use.getState().pending).toMatchObject([{ text: full }]);
    expect(readDraft('s1', 'sess')?.text).toBe('');
    expect(container.textContent).not.toContain('composer_voice_send_cancelled');
    await act(async () => vi.advanceTimersByTimeAsync(4000));
    expect(realFirstInput.send).toHaveBeenCalledTimes(1);
  });

  it('Enviar durante a transcrição arma uma intenção sem antecipar nem também iniciar os 3 segundos', async () => {
    const request = pendingTranscription();
    const { container } = await start();
    await act(async () => voiceInput.finishRecording!('silencio'));
    expect(realFirstInput.transcribe).toHaveBeenCalledTimes(1);
    expect(send(container).disabled).toBe(false);
    act(() => { send(container).click(); send(container).click(); });
    expect(realFirstInput.send).not.toHaveBeenCalled();
    await complete(request);
    expect(realFirstInput.send).toHaveBeenCalledExactlyOnceWith(useServers.getState().servers.find((s) => s.id === 's1'), 'sess', answer.text);
    expect(container.textContent).not.toContain('composer_enviando_cancelar');
    await act(async () => vi.advanceTimersByTimeAsync(4000));
    expect(realFirstInput.send).toHaveBeenCalledTimes(1);
  });

  it('Orientar com voz conserva o modo e não chama Enviar ao concluir', async () => {
    const request = pendingTranscription();
    const { container } = await start('antes');
    expect(steer(container).disabled).toBe(false);
    await act(async () => { steer(container).click(); send(container).click(); });
    expect(realFirstInput.steer).not.toHaveBeenCalled();
    await complete(request);
    expect(realFirstInput.steer).toHaveBeenCalledExactlyOnceWith('sess', `antes ${answer.text}`, useServers.getState().servers.find((s) => s.id === 's1'));
    expect(realFirstInput.send).not.toHaveBeenCalled();
  });

  it.each(['falha', 'vazio', 'aviso'])('%s cancela o envio e conserva texto e áudio para recuperar', async (result) => {
    const request = pendingTranscription();
    const { container } = await start('antes');
    await act(async () => send(container).click());
    if (result === 'falha') await act(async () => { request.fail(new Error('transcrição indisponível')); await voiceInput.finished; });
    else await complete(request, result === 'vazio' ? { ...answer, text: '' } : { ...answer, aviso: 'organização falhou: texto cru', text: answer.raw });
    expect(realFirstInput.send).not.toHaveBeenCalled();
    expect(realFirstInput.steer).not.toHaveBeenCalled();
    expect(readDraft('s1', 'sess')?.text).toBe(result === 'aviso' ? `antes ${answer.raw}` : 'antes');
    if (result !== 'aviso') expect(readDictation('s1', 'sess')).toMatchObject({ status: 'failed', serverPath: answer.path, audio: { uri: 'file:///cache/ditado.m4a' } });
    expect(container.textContent).toContain(result === 'falha' ? 'transcrição indisponível' : result === 'vazio' ? 'composer_transcricao_vazia' : 'organização falhou');
    expect(container.textContent).not.toContain('composer_voice_send_cancelled');
    await act(async () => vi.advanceTimersByTimeAsync(4000));
    expect(realFirstInput.send).not.toHaveBeenCalled();
  });

  it('erro da parada cancela a intenção mesmo se um resultado tardio chegar', async () => {
    const request = pendingTranscription();
    const { container } = await start('antes');
    await act(async () => send(container).click());
    act(() => voiceInput.onStopError!(new Error('ditado_parada_falhou')));
    await complete(request);
    expect(realFirstInput.send).not.toHaveBeenCalled();
    expect(container.textContent).toContain('composer_falha_gravacao');
    expect(readDraft('s1', 'sess')?.text).toBe(`antes ${answer.text}`);
  });

  it.each(['edição', 'revisão', 'blur', 'background', 'unmount', 'transcript'])('%s durante a espera cancela sem descartar o resultado', async (change) => {
    const request = pendingTranscription();
    const { container, root } = await start('antes');
    await act(async () => send(container).click());
    if (change === 'edição') { type(container, 'outro'); type(container, 'antes'); }
    if (change === 'revisão') act(() => {
      const latest = readDraft('s1', 'sess')!;
      writeDraft('s1', 'sess', { ...latest, revision: latest.revision + 1 });
      chatStore('s1', 'sess').use.setState((s) => ({ draftUpdate: s.draftUpdate + 1 }));
    });
    if (change === 'blur') act(() => {
      focusNavigation.focused = false;
      focusNavigation.listeners.get('blur')?.forEach((listener) => listener({ data: { closing: false } }));
    });
    if (change === 'background') act(() => {
      appLifecycle.currentState = 'background';
      appLifecycle.listeners.forEach((listener) => listener('background'));
    });
    if (change === 'unmount') close(root);
    if (change === 'transcript') {
      sessionsState.rows = [{ serverId: 's1', name: 'sess', jsonl: '/t/recreated.jsonl' }];
      await act(async () => root.render(createElement(Composer, props)));
    }
    if (change !== 'unmount') expect(container.textContent).toContain('composer_voice_send_cancelled');
    await complete(request);
    expect(realFirstInput.send).not.toHaveBeenCalled();
    expect(realFirstInput.steer).not.toHaveBeenCalled();
    expect(readDictation('s1', 'sess')).toMatchObject({ status: 'ready', text: answer.text, serverPath: answer.path });
    await act(async () => vi.advanceTimersByTimeAsync(4000));
    expect(realFirstInput.send).not.toHaveBeenCalled();
  });

  it.each(['send', 'steer', 'discard'] as const)('Cancelar com parada nativa atrasada (%s) preserva só o áudio de uma intenção de envio', async (mode) => {
    voiceInput.delayStop = true;
    const request = pendingTranscription();
    const { container } = await start('antes');
    if (mode !== 'discard') {
      await act(async () => (mode === 'send' ? send(container) : steer(container)).click());
      expect(voiceInput.stop).toHaveBeenCalledExactlyOnceWith('botao');
      expect(voiceInput.stopPending).not.toBeNull();
      expect(voiceInput.finished).toBeNull();
      expect(realFirstInput.upload).not.toHaveBeenCalled();
    }
    await act(async () => button(container, 'native_dictation_cancel')!.click());
    expect(voiceInput.finished).toBeNull();
    expect(realFirstInput.transcribe).not.toHaveBeenCalled();
    const stopping = voiceInput.stopPending;
    await act(async () => { voiceInput.finishStop!(); await stopping; });
    if (mode === 'discard') {
      expect(realFirstInput.upload).not.toHaveBeenCalled();
      expect(readDictation('s1', 'sess')).toBeNull();
      expect(readDraft('s1', 'sess')?.text).toBe('antes');
    } else {
      expect(realFirstInput.transcribe).toHaveBeenCalledTimes(1);
      expect(readDictation('s1', 'sess')).toMatchObject({ status: 'pending', serverPath: answer.path,
        audio: { uri: 'file:///cache/ditado.m4a' } });
      await complete(request);
      expect(readDraft('s1', 'sess')).toMatchObject({ text: `antes ${answer.text}`, dictationId: expect.any(String) });
      expect(container.querySelector('textarea')!.value).toBe(`antes ${answer.text}`);
    }
    expect(realFirstInput.send).not.toHaveBeenCalled();
    expect(realFirstInput.steer).not.toHaveBeenCalled();
  });

  it('Atualizar durante a transcrição faz só PATCH na fila e mantém rascunho e ditado separados', async () => {
    const panel: QueuePanel = { actions: true, entries: [{ id: 'q1', text: 'texto na fila', ts: null }] };
    const chat = chatStore('s1', 'sess');
    chat.use.setState({ queuePanel: panel });
    const previousServers = useServers.getState().servers;
    const source: import('@hangar/core').Server = { id: 's1', label: 'A', baseUrl: 'https://a.test', token: 'token-a' };
    useServers.getState().servers = previousServers.map((s) => s.id === source.id ? source : s);
    configureApi({ getBaseUrl: () => 'https://b.test', getToken: () => 'token-b', onUnauthorized: () => {}, origin: null,
      createEventSource: () => { throw new Error('unexpected SSE'); } });
    const network = vi.spyOn(globalThis, 'fetch').mockImplementation(async () => new Response(JSON.stringify({ ok: true }), {
      status: 200, headers: { 'Content-Type': 'application/json' },
    }));
    try {
      const request = pendingTranscription();
      const { container, root } = await render(createElement(Composer, { ...props, queuePanel: panel }));
      roots.add(root);
      type(container, 'rascunho antes da voz');
      await act(async () => button(container, 'composer_gravar_audio')!.click());
      await act(async () => voiceInput.finishRecording!('botao'));
      const voice = readDictation('s1', 'sess')!;
      expect(voice.status).toBe('pending');
      let queuedText = '';
      act(() => { queuedText = chat.startQueueEdit('q1', 'rascunho antes da voz'); });
      type(container, queuedText);
      type(container, 'fila revisada');
      const update = container.querySelector<HTMLButtonElement>('[aria-label="queue_panel_update_aria"]')!;
      expect(update.disabled).toBe(false);
      await act(async () => update.click());
      const patches = network.mock.calls.filter(([, options]) => options?.method === 'PATCH');
      expect(patches).toHaveLength(1);
      expect(patches[0][0]).toBe('https://a.test/api/sessions/sess/queue/q1');
      expect(JSON.parse(String(patches[0][1]?.body))).toEqual({ text: 'fila revisada' });
      expect(new Headers(patches[0][1]?.headers).get('Authorization')).toBe('Bearer token-a');
      expect(chat.use.getState().queueEdit).toBeNull();
      expect(readDraft('s1', 'sess')).toMatchObject({ text: 'rascunho antes da voz', submission: null });
      expect(readDictation('s1', 'sess')).toEqual(voice);
      expect(realFirstInput.send).not.toHaveBeenCalled();
      expect(realFirstInput.steer).not.toHaveBeenCalled();
      expect(network.mock.calls.some(([url]) => /\/(input|steer)$/.test(String(url)))).toBe(false);
      await complete(request);
      expect(readDraft('s1', 'sess')?.text).toBe('rascunho antes da voz');
      expect(readDictation('s1', 'sess')).toMatchObject({ id: voice.id, status: 'ready', text: answer.text });
      expect(realFirstInput.send).not.toHaveBeenCalled();
      expect(realFirstInput.steer).not.toHaveBeenCalled();
    } finally {
      network.mockRestore();
      useServers.getState().servers = previousServers;
    }
  });

  it.each(['anexo', 'grupo', 'pares'])('mudar %s durante a espera cancela a intenção', async (change) => {
    if (change === 'anexo') writeDraft('s1', 'sess', { version: 1, text: 'antes', revision: 1, transcript: '/t/voice.jsonl', submission: null,
      attachment: { uri: 'file:///foto.png', name: 'foto.png', mime: 'image/png', kind: 'image' } });
    sessionsState.rows = [{ serverId: 's1', name: 'sess', jsonl: '/t/voice.jsonl', pair_peers: ['par'] }];
    const request = pendingTranscription();
    const { container, root } = await start(change === 'anexo' ? '' : 'antes');
    await act(async () => send(container).click());
    if (change === 'anexo') act(() => button(container, 'board_remover_anexo')!.click());
    if (change === 'grupo') {
      act(() => button(container, 'composer_adicionar_ao_chat')!.click());
      act(() => button(container, 'send-to-group')!.click());
    }
    if (change === 'pares') {
      sessionsState.rows = [{ serverId: 's1', name: 'sess', jsonl: '/t/voice.jsonl', pair_peers: ['outro'] }];
      await act(async () => root.render(createElement(Composer, props)));
    }
    expect(container.textContent).toContain('composer_voice_send_cancelled');
    await complete(request);
    expect(realFirstInput.send).not.toHaveBeenCalled();
    expect(realFirstInput.steer).not.toHaveBeenCalled();
    expect(readDraft('s1', 'sess')?.text).toBe(`antes ${answer.text}`);
    expect(container.textContent).toContain('composer_voice_send_cancelled');
  });

  it.each(['turno encerrado', 'anexo', 'edição da fila'])('Orientar pendente com %s avisa sem virar envio comum', async (change) => {
    const panel: QueuePanel = { actions: true, entries: [{ id: 'q1', text: 'na fila', ts: null }] };
    const chat = chatStore('s1', 'sess');
    chat.use.setState({ queuePanel: panel });
    const request = pendingTranscription();
    const { container, root } = await start('antes');
    await act(async () => root.render(createElement(Composer, { ...props, queuePanel: panel })));
    expect(container.querySelectorAll('[aria-label="composer_steer_current_text"]')).toHaveLength(1);
    const queuedSteer = container.querySelectorAll('[aria-label="queue_panel_steer_message: na fila"]');
    expect(queuedSteer).toHaveLength(1);
    expect(steer(container).getAttribute('aria-label')).not.toBe(queuedSteer[0].getAttribute('aria-label'));
    expect(steer(container).textContent).toBe('codex_orientar');
    await act(async () => steer(container).click());
    let queuedText = '';
    act(() => {
      if (change === 'turno encerrado') chat.use.setState({ stateEvent: { session: 'sess', state: 'idle' } });
      if (change === 'edição da fila') queuedText = chat.startQueueEdit('q1', 'antes');
      if (change === 'anexo') {
        writeDraft('s1', 'sess', { ...readDraft('s1', 'sess')!,
          attachment: { uri: 'file:///foto.png', name: 'foto.png', mime: 'image/png', kind: 'image' } });
        chat.use.setState((s) => ({ draftUpdate: s.draftUpdate + 1 }));
      }
    });
    if (change === 'edição da fila') type(container, queuedText);
    await complete(request);
    expect(realFirstInput.send).not.toHaveBeenCalled();
    expect(realFirstInput.steer).not.toHaveBeenCalled();
    expect(container.textContent).toContain('composer_voice_send_cancelled');
    const kept = change === 'edição da fila' ? 'antes' : `antes ${answer.text}`;
    expect(container.querySelector('textarea')!.value).toBe(change === 'edição da fila' ? 'na fila' : kept);
    expect(readDraft('s1', 'sess')?.text).toBe(kept);
    if (change === 'anexo') expect(readDraft('s1', 'sess')?.attachment?.uri).toBe('file:///foto.png');
    if (change === 'edição da fila') expect(readDictation('s1', 'sess')).toMatchObject({ status: 'ready', text: answer.text });
  });

  it.each([false, true])('início do microfone pendente bloqueia Enviar e Orientar sem mandar o texto anterior, falha=%s', async (failed) => {
    let ready!: () => void;
    voiceInput.startPending = new Promise<void>((resolve) => { ready = resolve; });
    if (failed) voiceInput.startError = new Error('permission_denied');
    const { container } = await mount('antes');
    await act(async () => button(container, 'composer_gravar_audio')!.click());
    expect(send(container).disabled).toBe(true);
    expect(steer(container).disabled).toBe(true);
    act(() => { send(container).click(); steer(container).click(); });
    expect(realFirstInput.send).not.toHaveBeenCalled();
    expect(realFirstInput.steer).not.toHaveBeenCalled();
    expect(readDraft('s1', 'sess')?.text).toBe('antes');
    await act(async () => ready());
    expect(send(container).disabled).toBe(false);
    if (failed) {
      expect(container.textContent).toContain('composer_sem_acesso_mic');
      expect(button(container, 'composer_gravar_audio')).toBeDefined();
    } else expect(button(container, 'composer_parar_gravacao')).toBeDefined();
    expect(realFirstInput.send).not.toHaveBeenCalled();
  });

  it('editar e sair da tela sem intenção pendente não mostra cancelamento', async () => {
    const { container } = await mount('antes');
    type(container, 'editado');
    act(() => {
      focusNavigation.focused = false;
      focusNavigation.listeners.get('blur')?.forEach((listener) => listener({ data: { closing: false } }));
      appLifecycle.currentState = 'background';
      appLifecycle.listeners.forEach((listener) => listener('background'));
    });
    expect(container.textContent).not.toContain('composer_voice_send_cancelled');
    expect(readDraft('s1', 'sess')?.text).toBe('editado');
    expect(realFirstInput.send).not.toHaveBeenCalled();
  });

  it.each(['send', 'steer'] as const)('%s: parada sem onFim solta o botão, avisa e conserva o campo', async (mode) => {
    voiceInput.stopWithoutResult = true;
    const { container } = await start('antes');
    await act(async () => (mode === 'send' ? send(container) : steer(container)).click());
    expect(voiceInput.stop).toHaveBeenCalledExactlyOnceWith('botao');
    expect(voiceInput.finished).toBeNull();
    expect(realFirstInput.upload).not.toHaveBeenCalled();
    expect(realFirstInput.send).not.toHaveBeenCalled();
    expect(realFirstInput.steer).not.toHaveBeenCalled();
    expect(send(container).disabled).toBe(false);
    expect(container.textContent).toContain('composer_voice_send_cancelled');
    expect(container.querySelector('textarea')!.value).toBe('antes');
    expect(readDraft('s1', 'sess')?.text).toBe('antes');
  });

  it('pending externo órfão solta o botão no teto sem apagar texto nem áudio', async () => {
    writeDraft('s1', 'sess', { version: 1, text: 'antes', revision: 1, transcript: '/t/voice.jsonl', attachment: null, submission: null });
    setDictationInFlight('s1', 'sess', 'orphan', true);
    try {
      writeDictation('s1', 'sess', { version: 1, id: 'orphan', transcript: '/t/voice.jsonl', draftRevision: 1, before: 'antes',
        audio: { uri: 'file:///guardado.m4a', name: 'ditado.m4a', mime: 'audio/m4a', kind: 'file' },
        status: 'pending', motivo: 'botao', text: '', raw: '', issue: '' });
      const saved = readDictation('s1', 'sess');
      const { container } = await mount();
      act(() => send(container).click());
      expect(send(container).disabled).toBe(true);
      await act(async () => vi.advanceTimersByTimeAsync(299_999));
      expect(send(container).disabled).toBe(true);
      expect(container.textContent).not.toContain('composer_voice_send_cancelled');
      await act(async () => vi.advanceTimersByTimeAsync(1));
      expect(send(container).disabled).toBe(false);
      expect(container.textContent).toContain('composer_voice_send_cancelled');
      expect(readDraft('s1', 'sess')?.text).toBe('antes');
      expect(readDictation('s1', 'sess')).toEqual(saved);
      expect(realFirstInput.send).not.toHaveBeenCalled();
      expect(realFirstInput.steer).not.toHaveBeenCalled();
    } finally { setDictationInFlight('s1', 'sess', 'orphan', false); }
  });

  it('teto durante parada nativa desarma só o envio e aceita o ditado tardio', async () => {
    voiceInput.delayStop = true;
    const request = pendingTranscription();
    const { container } = await start('antes');
    await act(async () => send(container).click());
    await act(async () => vi.advanceTimersByTimeAsync(300_000));
    expect(send(container).disabled).toBe(false);
    expect(container.textContent).toContain('composer_voice_send_cancelled');
    const stopping = voiceInput.stopPending;
    await act(async () => { voiceInput.finishStop!(); await stopping; });
    expect(container.textContent).toContain('composer_voice_send_cancelled');
    await complete(request);
    expect(readDraft('s1', 'sess')?.text).toBe(`antes ${answer.text}`);
    expect(realFirstInput.send).not.toHaveBeenCalled();
    expect(realFirstInput.steer).not.toHaveBeenCalled();
    expect(container.textContent).toContain('composer_voice_send_cancelled');
  });

  it('trocar servidor cancela e avisa sem perder a transcrição da origem', async () => {
    const previousServers = useServers.getState().servers;
    try {
      const request = pendingTranscription();
      const { container, root } = await start('antes');
      await act(async () => send(container).click());
      useServers.getState().servers = previousServers.map((s) => s.id === 's1' ? { ...s, token: 'outro-token' } : s);
      await act(async () => root.render(createElement(Composer, props)));
      expect(container.textContent).toContain('composer_voice_send_cancelled');
      await complete(request);
      expect(readDraft('s1', 'sess')?.text).toBe(`antes ${answer.text}`);
      expect(container.textContent).toContain('composer_voice_send_cancelled');
      expect(realFirstInput.send).not.toHaveBeenCalled();
    } finally { useServers.getState().servers = previousServers; }
  });

  it('aviso sobrevive ao começo tardio da transcrição depois da cópia do áudio', async () => {
    let copied!: (value: import('../stores/drafts').DraftAttachment) => void;
    vi.mocked(retainDraftAttachment).mockImplementationOnce(() => new Promise((resolve) => { copied = resolve; }));
    const request = pendingTranscription();
    const { container, root } = await start('antes');
    await act(async () => send(container).click());
    sessionsState.rows = [{ serverId: 's1', name: 'sess', jsonl: '/t/voice.jsonl', pair_peers: ['outro'] }];
    await act(async () => root.render(createElement(Composer, props)));
    expect(container.textContent).toContain('composer_voice_send_cancelled');
    await act(async () => copied({ uri: 'file:///guardado.m4a', name: 'ditado.m4a', mime: 'audio/m4a', kind: 'file' }));
    expect(realFirstInput.transcribe).toHaveBeenCalledTimes(1);
    expect(container.textContent).toContain('composer_voice_send_cancelled');
    await complete(request);
    expect(container.querySelector('textarea')!.value).toBe(`antes ${answer.text}`);
    expect(container.textContent).toContain('composer_voice_send_cancelled');
    expect(realFirstInput.send).not.toHaveBeenCalled();
  });

  it('voltar ao foco e aplicar ditado pronto não apaga o aviso de cancelamento', async () => {
    const request = pendingTranscription();
    const { container } = await start('antes');
    await act(async () => send(container).click());
    act(() => {
      focusNavigation.focused = false;
      focusNavigation.listeners.get('blur')?.forEach((listener) => listener({ data: { closing: false } }));
    });
    await complete(request);
    expect(readDictation('s1', 'sess')?.status).toBe('ready');
    act(() => {
      focusNavigation.focused = true;
      chatStore('s1', 'sess').use.setState((s) => ({ draftUpdate: s.draftUpdate + 1 }));
    });
    expect(container.querySelector('textarea')!.value).toBe(`antes ${answer.text}`);
    expect(container.textContent).toContain('composer_voice_send_cancelled');
    expect(realFirstInput.send).not.toHaveBeenCalled();
  });

  it('cancelar envio pendente não rearma os 3 segundos; gravação nova pode autoenviar', async () => {
    const request = pendingTranscription();
    const { container, root } = await start();
    await act(async () => voiceInput.finishRecording!('silencio'));
    act(() => send(container).click());
    sessionsState.rows = [{ serverId: 's1', name: 'sess', jsonl: '/t/voice.jsonl', pair_peers: ['outro'] }];
    await act(async () => root.render(createElement(Composer, props)));
    await complete(request);
    expect(container.querySelector('textarea')!.value).toBe(answer.text);
    expect(container.textContent).toContain('composer_voice_send_cancelled');
    expect(container.textContent).not.toContain('composer_enviando_cancelar');
    await act(async () => vi.advanceTimersByTimeAsync(4000));
    expect(realFirstInput.send).not.toHaveBeenCalled();
    type(container, '');
    await act(async () => button(container, 'composer_gravar_audio')!.click());
    await act(async () => { voiceInput.finishRecording!('silencio'); await voiceInput.finished; });
    expect(container.textContent).toContain('composer_enviando_cancelar');
    expect(container.textContent).not.toContain('composer_voice_send_cancelled');
    await act(async () => vi.advanceTimersByTimeAsync(3000));
    expect(realFirstInput.send).toHaveBeenCalledExactlyOnceWith(useServers.getState().servers.find((s) => s.id === 's1'), 'sess', answer.text);
    expect(realFirstInput.steer).not.toHaveBeenCalled();
  });

  it.each([false, true])('transcrição de outra montagem só envia o mesmo resultado aplicado, aviso=%s', async (warning) => {
    const request = pendingTranscription();
    const first = await start('antes');
    await act(async () => voiceInput.finishRecording!('botao'));
    const finishing = voiceInput.finished;
    close(first.root);
    const back = await mount();
    expect(back.container.querySelector('textarea')!.value).toBe('antes');
    expect(send(back.container).disabled).toBe(false);
    act(() => { send(back.container).click(); send(back.container).click(); });
    expect(realFirstInput.send).not.toHaveBeenCalled();
    await act(async () => { request.done({ ...answer, aviso: warning ? 'organização falhou' : null }); await finishing; });
    if (warning) {
      expect(realFirstInput.send).not.toHaveBeenCalled();
      expect(back.container.querySelector('textarea')!.value).toBe(`antes ${answer.text}`);
    } else {
      expect(realFirstInput.send).toHaveBeenCalledExactlyOnceWith(useServers.getState().servers.find((s) => s.id === 's1'), 'sess', `antes ${answer.text}`);
      expect(back.container.querySelector('textarea')!.value).toBe('');
    }
  });
});

describe('par externo só leitura', () => {
  afterEach(() => { chatRow.extra = {}; });

  it('sessão de outra pessoa em ver e mandar recado não tem compositor', async () => {
    route.params = { server: 's1', name: 'sess' }; route.segments = ['s'];
    chatRow.extra = { guest_kind: 'pair' };
    const { container, root } = await render(createElement(ChatScreen));
    await act(async () => {});
    expect(container.textContent).toContain('native_par_so_leitura');
    expect(container.querySelector('textarea')).toBeNull();
    act(() => root.unmount());
  });
});

describe('handoff nas rotas reais', () => {
  beforeEach(() => {
    route.params = { server: 's1', name: 'sess' }; route.segments = ['s'];
    navigation.back.mockClear(); navigation.replace.mockClear(); navigation.canGoBack = true;
    composerChat.send.mockClear(); firstInput.send.mockClear(); firstInput.confirm.mockClear();
    firstInput.attempt = {
      id: 'attempt-route', serverId: 's1', body: { name: 'sess', cwd: '/repo', provider: 'claude' },
      sessionName: 'sess', text: 'rascunho recuperável', phase: 'created',
    };
  });

  it('chat recebe rascunho sem POST e a troca de servidor não carrega o texto antigo', async () => {
    const { container, root } = await render(createElement(ChatScreen));
    expect(container.querySelector('textarea')!.value).toBe('rascunho recuperável');
    expect(firstInput.send).not.toHaveBeenCalled();
    expect(composerChat.send).not.toHaveBeenCalled();
    route.params = { server: 's2', name: 'sess' };
    await act(async () => root.render(createElement(ChatScreen)));
    expect(container.querySelector('textarea')!.value).toBe('');
    expect(firstInput.confirm).not.toHaveBeenCalled();
    act(() => root.unmount());
  });

  it('chat confirma sent sem retornar texto ao campo nem enviar novamente', async () => {
    firstInput.attempt!.phase = 'sent';
    const { container, root } = await render(createElement(ChatScreen));
    expect(container.querySelector('textarea')!.value).toBe('');
    expect(firstInput.confirm).toHaveBeenCalledWith('attempt-route');
    expect(firstInput.send).not.toHaveBeenCalled();
    expect(composerChat.send).not.toHaveBeenCalled();
    act(() => root.unmount());
  });

  it.each([false, true])('ACK que chega após o handoff limpa somente o snapshot, edição nova: %s', async (edited) => {
    const { container, root } = await render(createElement(ChatScreen));
    const field = container.querySelector('textarea')!;
    if (edited) act(() => {
      Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value')!.set!.call(field, 'outra mensagem');
      field.dispatchEvent(new Event('input', { bubbles: true }));
    });
    firstInput.attempt = { ...firstInput.attempt!, phase: 'sent' };
    await act(async () => root.render(createElement(ChatScreen)));
    expect(field.value).toBe(edited ? 'outra mensagem' : '');
    expect(composerChat.send).not.toHaveBeenCalled();
    expect(firstInput.send).not.toHaveBeenCalled();
    act(() => root.unmount());
  });

  it('Nova conversa desmonta o formulário ao perder a rota para impedir navegação tardia', async () => {
    route.segments = ['create'];
    const { container, root } = await render(createElement(CreateRoute));
    expect(container.querySelector('[data-create]')).not.toBeNull();
    route.segments = ['s'];
    await act(async () => root.render(createElement(CreateRoute)));
    expect(container.querySelector('[data-create]')).toBeNull();
    act(() => root.unmount());
  });

  it.each([true, false])('Nova conversa tem saída acessível com ou sem tela anterior: %s', async (canGoBack) => {
    route.segments = ['create']; navigation.canGoBack = canGoBack;
    const { container, root } = await render(createElement(CreateRoute));
    const cancel = container.querySelector<HTMLButtonElement>('[aria-label="comum_cancelar"]');
    expect(cancel).not.toBeNull();
    expect(cancel!.style.minHeight).toBe('44px');
    expect(cancel!.style.minWidth).toBe('44px');
    act(() => cancel!.click());
    if (canGoBack) {
      expect(navigation.back).toHaveBeenCalledTimes(1);
      expect(navigation.replace).not.toHaveBeenCalled();
    } else {
      expect(navigation.replace).toHaveBeenCalledExactlyOnceWith('/');
      expect(navigation.back).not.toHaveBeenCalled();
    }
    act(() => root.unmount());
  });
});

describe('plano proposto na lista', () => {
  it('Codex: a bolha recebe o texto original, com os marcadores, pra detectar o plano', async () => {
    bubbleTexts.length = 0;
    const text = 'Antes\n<proposed_plan>\n# Plano\n- passo\n</proposed_plan>';
    const { root } = await render(createElement(MessageList, {
      events: [{ id: 'a1', kind: 'assistant_msg', text }],
      preview: '', olderFailed: '', onLoadOlder: () => {},
      session: { provider: 'codex' } as never,
    } as never));
    expect(bubbleTexts).toEqual([text]);
    act(() => root.unmount());
  });
});

describe('bloco trabalhando no fim da lista', () => {
  it('texto e tempo do rótulo do terminal e subagente rodando grudado no fim', async () => {
    const { container, root } = await render(createElement(MessageList, {
      events: [{ id: 'u1', kind: 'user_msg', text: 'vai', ts: Date.now() / 1000 - 12 },
               { id: 'a1', kind: 'tool_use', tool_name: 'Agent', tool_use_id: 'tu1', ts: Date.now() / 1000 - 5, tool_input: { description: 'Revisar diff' } }],
      preview: '', olderFailed: '', onLoadOlder: () => {},
      stateEvent: { session: 'sess', state: 'working', label: 'Pensando… (3s · esc to interrupt)' },
      agentesRodando: [{ id: 'tu1', kind: 'agent', description: 'Revisar diff', running: true, subagentType: 'Explore' }],
    } as never));
    expect(container.textContent).toContain('Pensando…');
    expect(container.textContent).not.toContain('esc to interrupt');
    // O tempo do terminal vence o contado no app: vale mesmo com o envio fora da janela carregada.
    expect(container.textContent).toContain('3s');
    expect(container.textContent).not.toContain('12 s');
    expect(container.textContent).toContain('Revisar diff');
    expect(container.textContent).toContain('Explore');
    act(() => root.unmount());
  });

  it('raciocínio em voo ocupa o lugar da linha de spinner', async () => {
    const { container, root } = await render(createElement(MessageList, {
      events: [], preview: '', olderFailed: '', onLoadOlder: () => {},
      stateEvent: { session: 'sess', state: 'working', label: null },
      pensamento: 'considerando as opções',
    } as never));
    expect(container.textContent).toContain('pensamento_vivo');
    expect(container.textContent).toContain('considerando as opções');
    expect(container.textContent).not.toContain('native_working_line');
    act(() => root.unmount());
  });
});

it('plano real preserva prosa, links e abertura do arquivo sem tags do protocolo', async () => {
  const { AssistantBubble } = await vi.importActual<typeof import('./AssistantBubble')>('./AssistantBubble');
  routerPush.mockClear();
  const text = 'Prosa antes\n<proposed_plan>\n# Plano Android\n[Documentação](https://docs.example.test/android)\n[Arquivo do plano](/repo/docs/plano.md)\n</proposed_plan>\nProsa depois';
  const { container, root } = await render(createElement(AssistantBubble, { text, sessionName: 'sess', serverId: 'srv' }));
  const markdown = container.querySelector('pre')!.textContent;
  expect(markdown).toContain('Prosa antes');
  expect(markdown).toContain('Prosa depois');
  expect(markdown).toContain('[Documentação](https://docs.example.test/android)');
  expect(markdown).toContain('[Arquivo do plano](/repo/docs/plano.md)');
  expect(markdown).not.toContain('proposed_plan');
  expect(container.textContent).toContain('chat_plan_proposto');
  const chip = container.querySelector<HTMLButtonElement>('[aria-label="/repo/docs/plano.md"]');
  expect(chip).not.toBeNull();
  act(() => chip!.click());
  expect(routerPush).toHaveBeenCalledExactlyOnceWith('/s/srv/sess/files?path=%2Frepo%2Fdocs%2Fplano.md');
  act(() => root.unmount());
});

describe('ditado entregue à conversa de origem', () => {
  const props = { serverId: 's1', name: 'sess', queuePanel: noQueue };
  const type = (container: HTMLElement, value: string) => act(() => {
    const field = container.querySelector('textarea')!;
    Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value')!.set!.call(field, value);
    field.dispatchEvent(new Event('input', { bubbles: true }));
  });
  const mic = (c: HTMLElement) => c.querySelector<HTMLButtonElement>('[aria-label="composer_gravar_audio"]')!;
  const audio = { uri: 'file:///doc/draft-attachments/1-1.m4a', name: 'ditado.m4a', mime: 'audio/m4a', kind: 'file' };
  const voiceRecord = (patch: Record<string, unknown> = {}) => ({
    version: 1, id: 'v1', audio, transcript: '/t/a.jsonl', draftRevision: 2, before: 'antes', motivo: 'botao',
    status: 'ready', text: 'ditado', raw: 'ditado cru', issue: '', ...patch,
  });
  const storeDraft = (patch: Record<string, unknown> = {}) => storage.memory.set('draft.v1:s1::sess', JSON.stringify({
    version: 1, text: 'antes', revision: 2, transcript: '/t/a.jsonl', attachment: null, submission: null, ...patch,
  }));
  beforeEach(() => { sessionsState.rows = [{ serverId: 's1', name: 'sess', jsonl: '/t/a.jsonl' }]; });

  it('ditado pronto com o rascunho intacto entra sozinho no fim ao montar, sem botão de recuperar', async () => {
    storeDraft();
    storage.memory.set('draft.v1.dictation:s1::sess', JSON.stringify(voiceRecord()));
    const { container, root } = await render(createElement(Composer, props));
    expect(container.querySelector('textarea')!.value).toBe('antes ditado');
    expect(button(container, 'composer_draft_recover')).toBeUndefined();
    expect(storage.memory.has('draft.v1.dictation:s1::sess')).toBe(false);
    act(() => root.unmount());
  });

  it('rascunho mexido depois da gravação mantém o botão de recuperar e não mexe no campo', async () => {
    storeDraft({ text: 'antes editado', revision: 3 });
    storage.memory.set('draft.v1.dictation:s1::sess', JSON.stringify(voiceRecord()));
    const { container, root } = await render(createElement(Composer, props));
    expect(container.querySelector('textarea')!.value).toBe('antes editado');
    expect(button(container, 'composer_draft_recover')).toBeDefined();
    expect(storage.memory.has('draft.v1.dictation:s1::sess')).toBe(true);
    act(() => root.unmount());
  });

  it('sessão recriada com o mesmo nome não recebe o ditado sozinha', async () => {
    sessionsState.rows = [{ serverId: 's1', name: 'sess', jsonl: '/t/new.jsonl' }];
    storeDraft();
    storage.memory.set('draft.v1.dictation:s1::sess', JSON.stringify(voiceRecord()));
    const { container, root } = await render(createElement(Composer, props));
    expect(container.querySelector('textarea')!.value).toBe('');
    expect(container.textContent).toContain('composer_ditado_recuperavel');
    expect(JSON.parse(storage.memory.get('draft.v1.dictation:s1::sess')!).status).toBe('ready');
    act(() => root.unmount());
  });

  it('trocar de conversa logo depois de parar ainda transcreve para a origem, e o texto entra ao voltar', async () => {
    realFirstInput.transcribe.mockClear().mockResolvedValueOnce({ path: '/up/ditado-1.m4a', text: 'ditado', raw: 'ditado', aviso: null });
    const first = await render(createElement(Composer, props));
    type(first.container, 'antes');
    await act(async () => mic(first.container).click());
    await act(async () => {
      const done = voiceInput.onFim!(new File(['a'], 'ditado.m4a', { type: 'audio/m4a' }), 'escondeu', 'file:///cache/ditado.m4a');
      first.root.unmount();
      await done;
    });
    expect(realFirstInput.transcribe).toHaveBeenCalledTimes(1);
    const back = await render(createElement(Composer, props));
    expect(back.container.querySelector('textarea')!.value).toBe('antes ditado');
    expect(button(back.container, 'composer_draft_recover')).toBeUndefined();
    act(() => back.root.unmount());
  });

  it('voltar no meio da transcrição mostra o microfone ocupado; o resultado entra ao voltar de novo', async () => {
    let finish!: (r: { path: string; text: string; raw: string; aviso: null }) => void;
    realFirstInput.transcribe.mockClear().mockImplementationOnce(() => new Promise((resolve) => { finish = resolve; }));
    const first = await render(createElement(Composer, props));
    type(first.container, 'antes');
    await act(async () => mic(first.container).click());
    await act(async () => { void voiceInput.onFim!(new File(['a'], 'ditado.m4a', { type: 'audio/m4a' }), 'botao', 'file:///cache/ditado.m4a'); });
    act(() => first.root.unmount());
    const back = await render(createElement(Composer, props));
    expect(mic(back.container).getAttribute('aria-busy')).toBe('true');
    expect(button(back.container, 'composer_transcrever_de_novo')).toBeUndefined();
    await act(async () => finish({ path: '/up/ditado-1.m4a', text: 'ditado', raw: 'ditado', aviso: null }));
    act(() => back.root.unmount());
    const again = await render(createElement(Composer, props));
    expect(again.container.querySelector('textarea')!.value).toBe('antes ditado');
    expect(realFirstInput.transcribe).toHaveBeenCalledTimes(1);
    act(() => again.root.unmount());
  });

  it('remontar durante a cópia do áudio: a cópia antiga não grava por cima do ditado novo no ar', async () => {
    let copied!: () => void;
    vi.mocked(retainDraftAttachment).mockImplementationOnce((a) => new Promise((resolve) => { copied = () => resolve(a); }));
    let finish!: (r: { path: string; text: string; raw: string; aviso: null }) => void;
    realFirstInput.transcribe.mockClear().mockImplementationOnce(() => new Promise((resolve) => { finish = resolve; }));
    const first = await render(createElement(Composer, props));
    await act(async () => mic(first.container).click());
    await act(async () => { void voiceInput.onFim!(new File(['a'], 'a.m4a', { type: 'audio/m4a' }), 'escondeu', 'file:///cache/a.m4a'); });
    act(() => first.root.unmount());
    const second = await render(createElement(Composer, props));
    await act(async () => mic(second.container).click());
    await act(async () => { void voiceInput.onFim!(new File(['b'], 'b.m4a', { type: 'audio/m4a' }), 'botao', 'file:///cache/b.m4a'); });
    await act(async () => copied());
    expect(JSON.parse(storage.memory.get('draft.v1.dictation:s1::sess')!).audio.uri).toBe('file:///cache/b.m4a');
    expect(realFirstInput.transcribe).toHaveBeenCalledTimes(1);
    await act(async () => finish({ path: '/up/sess/b.m4a', text: 'b', raw: 'b', aviso: null }));
    act(() => second.root.unmount());
  });

  it('microfone de outra montagem não abre enquanto o POST da conversa está no ar', async () => {
    let finish!: (r: { path: string; text: string; raw: string; aviso: null }) => void;
    realFirstInput.transcribe.mockClear().mockImplementationOnce(() => new Promise((resolve) => { finish = resolve; }));
    // A segunda montagem nasce antes do ditado e não relê o armazenamento (o mock do chat não avisa).
    const second = await render(createElement(Composer, props));
    const first = await render(createElement(Composer, props));
    await act(async () => mic(first.container).click());
    await act(async () => { void voiceInput.onFim!(new File(['a'], 'a.m4a', { type: 'audio/m4a' }), 'botao', 'file:///cache/a.m4a'); });
    await act(async () => mic(second.container).click());
    expect(second.container.textContent).toContain('composer_aguarde_transcricao');
    await act(async () => finish({ path: '/up/sess/a.m4a', text: 'a', raw: 'a', aviso: null }));
    act(() => { first.root.unmount(); second.root.unmount(); });
  });

  it('"Transcrever de novo" usa o arquivo já enviado pelo nome, sem subir outra cópia', async () => {
    realFirstInput.upload.mockClear();
    realFirstInput.transcribe.mockClear().mockResolvedValueOnce({ path: '/up/sess/ditado-1.m4a', text: 'de novo', raw: 'de novo', aviso: null });
    storeDraft({ text: '', revision: 1 });
    storage.memory.set('draft.v1.dictation:s1::sess', JSON.stringify(voiceRecord({
      status: 'failed', text: '', raw: '', issue: '502: groq', draftRevision: 1, before: '', serverPath: '/up/sess/ditado-1.m4a',
    })));
    const { container, root } = await render(createElement(Composer, props));
    await act(async () => button(container, 'composer_transcrever_de_novo')!.click());
    // Mesma conversa: nome solto, que o convidado também pode usar.
    expect(realFirstInput.transcribe).toHaveBeenCalledExactlyOnceWith({ id: 's1' }, 'sess', 'ditado-1.m4a', { limpar: true, estilo: undefined });
    expect(realFirstInput.upload).not.toHaveBeenCalled();
    expect(container.querySelector('textarea')!.value).toBe('de novo');
    act(() => root.unmount());
  });

  it('falha da transcrição deixa o "de novo" apontando para o arquivo já enviado', async () => {
    realFirstInput.upload.mockClear();
    realFirstInput.transcribe.mockClear().mockRejectedValueOnce(Object.assign(new Error('502: groq'), { status: 502 }));
    const { container, root } = await render(createElement(Composer, props));
    await act(async () => mic(container).click());
    await act(async () => voiceInput.onFim!(new File(['a'], 'ditado.m4a', { type: 'audio/m4a' }), 'botao', 'file:///cache/ditado.m4a'));
    expect(realFirstInput.upload).toHaveBeenCalledExactlyOnceWith({ id: 's1' }, 'sess', expect.any(File), { audioOnly: true });
    expect(JSON.parse(storage.memory.get('draft.v1.dictation:s1::sess')!)).toMatchObject({
      status: 'failed', serverPath: '/up/sess/ditado-1.m4a', issue: '502: groq',
    });
    expect(button(container, 'composer_transcrever_de_novo')).toBeDefined();
    act(() => root.unmount());
  });

  it('depois de /clear, o "de novo" pelo caminho absoluto guardado volta com "Recuperar", sem entrar sozinho', async () => {
    sessionsState.rows = [{ serverId: 's1', name: 'sess', jsonl: '/t/clear.jsonl' }];
    realFirstInput.upload.mockClear();
    realFirstInput.transcribe.mockClear().mockResolvedValueOnce({ path: '/up/sess/ditado-1.m4a', text: 'antigo', raw: 'antigo', aviso: null });
    storeDraft({ text: '', revision: 1, transcript: '/t/clear.jsonl' });
    storage.memory.set('draft.v1.dictation:s1::sess', JSON.stringify(voiceRecord({
      status: 'failed', text: '', raw: '', issue: '502: groq', draftRevision: 1, before: '', serverPath: '/up/sess/ditado-1.m4a',
    })));
    const { container, root } = await render(createElement(Composer, props));
    await act(async () => button(container, 'composer_transcrever_de_novo')!.click());
    expect(realFirstInput.transcribe).toHaveBeenCalledExactlyOnceWith({ id: 's1' }, 'sess', '/up/sess/ditado-1.m4a', { limpar: true, estilo: undefined });
    expect(realFirstInput.upload).not.toHaveBeenCalled();
    expect(container.querySelector('textarea')!.value).toBe('');
    expect(container.textContent).toContain('composer_ditado_recuperavel');
    expect(button(container, 'composer_draft_recover')).toBeDefined();
    act(() => root.unmount());
  });

  it('barra do ditado: cru sem rede, estilo pelo relimpar, edição fora do ditado recusa a troca', async () => {
    realFirstInput.transcribe.mockClear().mockResolvedValueOnce({
      path: '/up/sess/ditado-1.m4a', text: 'texto prosa', raw: 'texto cru', aviso: null, estilo_aplicado: 'prosa',
    });
    realFirstInput.relimpar.mockClear().mockResolvedValueOnce({ text: 'texto limpo', aviso: null, estilo_aplicado: 'limpar' });
    const { container, root } = await render(createElement(Composer, props));
    type(container, 'antes');
    await act(async () => mic(container).click());
    await act(async () => voiceInput.onFim!(new File(['a'], 'ditado.m4a', { type: 'audio/m4a' }), 'botao', 'file:///cache/ditado.m4a'));
    expect(container.querySelector('textarea')!.value).toBe('antes texto prosa');
    // A barra é só as quatro versões (e o player): sem botão de fechar, como no PC.
    const labels = ['composer_ditado_cru', ...estilosDitado().map((e) => e.rotulo)];
    const row = button(container, 'composer_ditado_cru')!.parentElement!;
    expect([...row.querySelectorAll('button')].map((b) => b.getAttribute('aria-label'))).toEqual(labels);
    const selected = () => [...row.querySelectorAll('button[aria-selected="true"]')].map((b) => b.getAttribute('aria-label'));
    const prosa = estilosDitado().find((e) => e.valor === 'prosa')!.rotulo;
    expect(selected()).toEqual([prosa]);
    expect(row.querySelector('[data-audio$="/uploads/ditado-1.m4a"]')).not.toBeNull();
    // O "Cru" faz o papel do desfazer: os dois não aparecem juntos.
    expect(button(container, 'composer_desfazer_limpeza')).toBeUndefined();
    await act(async () => button(container, 'composer_ditado_cru')!.click());
    expect(container.querySelector('textarea')!.value).toBe('antes texto cru');
    expect(realFirstInput.relimpar).not.toHaveBeenCalled();
    expect(selected()).toEqual(['composer_ditado_cru']);
    const limpar = estilosDitado().find((e) => e.valor === 'limpar')!.rotulo;
    await act(async () => button(container, limpar)!.click());
    expect(realFirstInput.relimpar).toHaveBeenCalledExactlyOnceWith('texto cru', 'limpar', { id: 's1' }, { session: 'sess' });
    expect(container.querySelector('textarea')!.value).toBe('antes texto limpo');
    expect(selected()).toEqual([limpar]);
    // Texto ditado intacto: as versões ficam, mas a troca recusa para não perder a edição.
    type(container, 'antes texto limpo e mais');
    await act(async () => button(container, 'composer_ditado_cru')!.click());
    expect(container.querySelector('textarea')!.value).toBe('antes texto limpo e mais');
    expect(container.textContent).toContain('native_dictation_draft_changed');
    // Mexer no próprio texto ditado esconde as versões e o player juntos.
    type(container, 'antes outro texto');
    expect(button(container, 'composer_ditado_cru')).toBeUndefined();
    expect(container.querySelector('[data-audio]')).toBeNull();
    act(() => root.unmount());
  });

  it('trocar de versão durante a contagem do envio automático cancela o envio', async () => {
    vi.useFakeTimers();
    try {
      composerChat.send.mockClear();
      realFirstInput.transcribe.mockClear().mockResolvedValueOnce({
        path: '/up/sess/ditado-1.m4a', text: 'texto prosa', raw: 'texto cru', aviso: null, estilo_aplicado: 'prosa',
      });
      const { container, root } = await render(createElement(Composer, props));
      await act(async () => mic(container).click());
      await act(async () => voiceInput.onFim!(new File(['a'], 'ditado.m4a', { type: 'audio/m4a' }), 'silencio', 'file:///cache/ditado.m4a'));
      expect(container.textContent).toContain('composer_enviando_cancelar');
      await act(async () => button(container, 'composer_ditado_cru')!.click());
      expect(container.querySelector('textarea')!.value).toBe('texto cru');
      expect(container.textContent).not.toContain('composer_enviando_cancelar');
      await act(async () => vi.advanceTimersByTimeAsync(3250));
      expect(composerChat.send).not.toHaveBeenCalled();
      act(() => root.unmount());
    } finally {
      vi.useRealTimers();
    }
  });

  it('enviar fecha a barra do ditado', async () => {
    realFirstInput.transcribe.mockClear().mockResolvedValueOnce({
      path: '/up/sess/ditado-1.m4a', text: 'texto prosa', raw: 'texto cru', aviso: null, estilo_aplicado: 'prosa',
    });
    const { container, root } = await render(createElement(Composer, props));
    await act(async () => mic(container).click());
    await act(async () => voiceInput.onFim!(new File(['a'], 'ditado.m4a', { type: 'audio/m4a' }), 'botao', 'file:///cache/ditado.m4a'));
    expect(button(container, 'composer_ditado_cru')).toBeDefined();
    await act(async () => container.querySelector<HTMLButtonElement>('[aria-label="composer_enviar_mensagem"]')!.click());
    expect(button(container, 'composer_ditado_cru')).toBeUndefined();
    // O campo vazio já esconde a barra; só o mesmo texto de volta prova que ela foi solta no envio.
    type(container, 'texto prosa');
    expect(button(container, 'composer_ditado_cru')).toBeUndefined();
    act(() => root.unmount());
  });
});
