// @vitest-environment happy-dom
import { act, createElement, StrictMode, useEffect, useRef, useState, type ReactNode } from 'react';
import { createRoot } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const calls = vi.hoisted(() => ({
  roots: vi.fn(), scan: vi.fn(), sessions: vi.fn(), configs: vi.fn(), engines: vi.fn(), save: vi.fn(),
  startDictation: vi.fn(), stopDictation: vi.fn(), transcribe: vi.fn(),
  activeId: 'server-b', startPending: null as Promise<void> | null,
  target: null as null | { id: string; label: string; baseUrl: string; token: string },
  accounts: vi.fn(),
  providers: vi.fn(),
  mutate: vi.fn(),
  contextFetch: vi.fn(),
  models: vi.fn(),
  prepare: vi.fn(),
  preparation: vi.fn(),
  create: vi.fn(),
  send: vi.fn(),
  history: vi.fn(),
  archives: vi.fn(),
  resume: vi.fn(),
  replace: vi.fn(),
  alert: vi.fn(),
  dismiss: null as null | (() => void),
  pickerActive: 0,
  pickerPeak: 0,
  pickerHistory: [] as number[],
  cotas: [] as unknown[],
}));
const server = { id: 'server-b', label: 'Servidor B', baseUrl: 'https://b.local', token: 'token-b' };

vi.mock('expo-router', () => ({ useRouter: () => ({ replace: calls.replace }) }));
vi.mock('react-native', async (original) => {
  const actual = await original<typeof import('react-native')>();
  return {
  ...actual,
  AccessibilityInfo: { ...actual.AccessibilityInfo, sendAccessibilityEvent: vi.fn(), announceForAccessibility: vi.fn() },
  Switch: ({ value, onValueChange, disabled, accessibilityLabel }: { value: boolean; onValueChange: (v: boolean) => void; disabled?: boolean; accessibilityLabel?: string }) =>
    createElement('button', { role: 'switch', 'aria-label': accessibilityLabel, 'aria-checked': value, disabled, onClick: () => onValueChange(!value) }),
  // O mock global só achata arrays; o estilo por função do Pressable precisa ser resolvido antes.
  Pressable: (props: { style?: unknown; onLongPress?: () => void; accessibilityState?: { expanded?: boolean } }) => (actual.Pressable as unknown as (p: object) => ReactNode)({
    ...props, onContextMenu: props.onLongPress, 'aria-expanded': props.accessibilityState?.expanded,
    style: typeof props.style === 'function' ? props.style({ pressed: false }) : props.style,
  }),
  Alert: { alert: calls.alert },
  TextInput: (props: { ref?: React.Ref<HTMLTextAreaElement>; value?: string; accessibilityLabel?: string; onChangeText?: (value: string) => void }) => createElement('textarea', {
    ref: props.ref,
    value: props.value,
    'aria-label': props.accessibilityLabel,
    onInput: (event: { currentTarget: { value: string } }) => props.onChangeText?.(event.currentTarget.value),
  }),
  };
});
vi.mock('react-native-keyboard-controller', () => ({ KeyboardAvoidingView: ({ children }: { children: ReactNode }) => createElement('div', null, children) }));
// A folha agora monta a NewConversation, que importa o seletor de anexos.
vi.mock('expo-image-picker', () => ({}));
vi.mock('expo-document-picker', () => ({}));
vi.mock('../../chat/draftAttachments', () => ({ retainDraftAttachment: vi.fn(async (a: unknown) => a), removeDraftAttachment: vi.fn() }));
vi.mock('../ditado/EstiloPill', () => ({
  DictationStyleMenu: ({ open }: { open: boolean }) => open ? createElement('div', { role: 'dialog', 'aria-label': 'ditado_estilo_titulo' }) : null,
  useDictationStyleLabel: () => 'estilo',
}));
vi.mock('../usage/HomeUsageCard', async (original) => ({ ...await original<object>(), HomeUsageCard: () => createElement('div', { 'data-home-usage': true }) }));
vi.mock('../../ui/AttachSheet', () => ({ AttachSheet: ({ open, onPick, onClose }: { open: boolean; onPick: (attachment: unknown) => void; onClose: () => void }) => open ? createElement('button', {
  'aria-label': 'pick-fixture', onClick: () => { onPick({ uri: 'file:///fixture.png', name: 'fixture.png', type: 'image/png', kind: 'image' }); onClose(); },
}) : null }));
vi.mock('../ditado/useDitado', () => ({ useDitado: ({ onFim }: { onFim: (file: File) => void }) => {
  const [gravando, setGravando] = useState(false);
  return { gravando, rms: 0, iniciar: async () => { calls.startDictation(); if (calls.startPending) await calls.startPending; setGravando(true); }, parar: async () => {
    calls.stopDictation(); setGravando(false); onFim(new File(['audio'], 'fixture.wav', { type: 'audio/wav' }));
  } };
} }));
vi.mock('../../ui/AnchoredPanel', () => ({
  AnchoredPanel: ({ open, children, label, onClose, onDismissed }: {
    open: boolean; children: ReactNode; label: string; onClose: () => void; onDismissed?: () => void;
  }) => {
    const [retained, setRetained] = useState(open);
    const callback = useRef(onDismissed);
    callback.current = onDismissed;
    useEffect(() => {
      if (label !== 'native_new_chat_folder') return;
      if (open) setRetained(true);
      else if (retained) calls.dismiss = () => { setRetained(false); calls.dismiss = null; callback.current?.(); };
    }, [open, retained, label]);
    const shown = open || (label === 'native_new_chat_folder' && retained);
    return shown ? createElement('div', { 'data-testid': 'folder-panel', 'aria-label': label,
      'data-folder-panel': label === 'native_new_chat_folder' ? 'true' : undefined, 'data-open': String(open) }, children,
      createElement('button', { onClick: onClose, 'aria-label': 'close-panel' }),
      createElement('button', { onClick: onClose, 'aria-label': 'back-panel' })) : null;
  },
}));
vi.mock('./CwdPicker', async (original) => {
  const { CwdPicker } = await original<typeof import('./CwdPicker')>();
  return { CwdPicker: (props: Parameters<typeof CwdPicker>[0]) => {
    useEffect(() => {
      calls.pickerActive++;
      calls.pickerPeak = Math.max(calls.pickerPeak, calls.pickerActive);
      calls.pickerHistory.push(calls.pickerActive);
      return () => { calls.pickerActive--; calls.pickerHistory.push(calls.pickerActive); };
    }, []);
    return createElement('div', { 'data-testid': 'cwd-picker' }, createElement(CwdPicker, props));
  } };
});
vi.mock('../../stores/servers', () => ({
  useServers: Object.assign(
    // A máquina da conversa sai da pílula de máquina, não do servidor ativo: `target` só entra na lista.
    (selector: (state: { activeId: string; servers: typeof server[] }) => unknown) =>
      selector({ activeId: calls.activeId, servers: calls.target ? [server, calls.target] : [server] }),
    { getState: () => ({ servers: calls.target ? [server, calls.target] : [server], active: () => server }) },
  ),
}));
vi.mock('../../stores/prefs', () => ({ prefs: {
  getString: (key: string) => localStorage.getItem(key) ?? undefined,
  set: (key: string, value: string) => { calls.save(key, value); localStorage.setItem(key, value); },
  remove: (key: string) => localStorage.removeItem(key),
} }));
vi.mock('../../ui/Icon', () => ({ Icon: () => null }));
vi.mock('../../ui/HangarMark', () => ({ HangarMark: () => null }));
vi.mock('../../ui/Glass', () => ({ Glass: ({ children }: { children: ReactNode }) => createElement('div', null, children) }));
vi.mock('@react-native-menu/menu', () => ({
  MenuView: () => { throw new Error('creation-must-use-anchored-panel'); },
}));
vi.mock('@hangar/core', async (original) => ({
  ...await original<typeof import('@hangar/core')>(),
  transcribeDictationForServer: calls.transcribe,
  listClaudeConfigsForServer: calls.configs,
  getRootsForServer: calls.roots, scanDirForServer: calls.scan, fetchSessionsForServer: calls.sessions,
  probeServerResponse: vi.fn().mockImplementation(async () => new Response(JSON.stringify(calls.cotas))),
  getEnginesForServer: calls.engines,
  // Sonda de providers da folha nova: sem ela o teste fazia fetch de verdade para b.local.
  getProvidersForServer: calls.providers,
  setServiceTier: calls.mutate, setModelEffort: calls.mutate, setCodexModel: calls.mutate,
  setEngineModel: calls.mutate, setPiModel: calls.mutate, setKimiModel: calls.mutate,
  setPermissionMode: calls.mutate,
  modelOptions: vi.fn().mockResolvedValue({ models: [], reduced: false }),
  getSessions: vi.fn().mockResolvedValue([]),
  getCodexAccountsForServer: calls.accounts,
  modelOptionsForServer: calls.models,
  prepareCodexAccountForServer: calls.prepare,
  getCodexPreparationForServer: calls.preparation,
  createSessionForServer: calls.create,
  sendInputForServer: calls.send,
  getHistory: calls.history,
  getArchivePorCwd: calls.archives,
  resumeArchivedConversation: calls.resume,
}));
vi.mock('../../paraglide/messages', () => ({
  codex_ui_account: () => 'codex_ui_account', codex_ui_login_error: () => 'codex_ui_login_error',
  codex_ui_prepare_error: () => 'codex_ui_prepare_error', codex_ui_unknown: () => 'codex_ui_unknown',
  codex_ui_abrindo_sessao: () => 'codex_ui_abrindo_sessao',
  criar_subagente: () => 'criar_subagente', criar_subagente_padrao: () => 'criar_subagente_padrao',
  criar_subagente_ajuda: () => 'criar_subagente_ajuda',
  composer_esforco: () => 'composer_esforco', composer_modelo: () => 'composer_modelo',
  comum_carregando: () => 'comum_carregando', comum_conta_claude: () => 'comum_conta_claude',
  comum_motor: () => 'comum_motor', comum_nome: () => 'comum_nome', comum_provider: () => 'comum_provider', nova_conversa_opcoes_fechar: () => 'nova_conversa_opcoes_fechar',
  contas_nao_conectada: () => 'contas_nao_conectada', cota_conta_parada: () => 'cota_conta_parada',
  cota_precisa_entrar: () => 'cota_precisa_entrar', cota_sem_cota: () => 'cota_sem_cota',
  criar_abre_padrao: ({ erro }: { erro: string }) => `criar_abre_padrao:${erro}`, criar_avancado: () => 'criar_avancado',
  criar_caminho_placeholder: () => 'criar_caminho_placeholder', criar_claude_sua_conta: () => 'criar_claude_sua_conta',
  criar_conta_aria: () => 'criar_conta_aria', criar_criando: () => 'criar_criando', criar_ja_existe: () => 'criar_ja_existe',
  criar_lista_reduzida: () => 'criar_lista_reduzida', criar_modelos_erro: () => 'criar_modelos_erro',
  criar_nome_placeholder: () => 'criar_nome_placeholder', criar_outra_pasta: () => 'criar_outra_pasta',
  criar_padrao: () => 'criar_padrao', criar_permissao: () => 'criar_permissao', criar_permissao_padrao: () => 'criar_permissao_padrao',
  criar_raciocinio: () => 'criar_raciocinio', criar_retomar: () => 'criar_retomar', criar_retomar_acao: () => 'criar_retomar_acao',
  criar_retomar_escolha: () => 'criar_retomar_escolha', criar_sessao_erro: () => 'criar_sessao_erro', criar_usar: () => 'criar_usar',
  criar_verificando: () => 'criar_verificando', criar_sessao: () => 'criar_sessao', sessao_nova: () => 'sessao_nova',
  switcher_atual: () => 'switcher_atual',
  criar_projeto_indisponivel: () => 'projeto_indisponivel', criar_projeto_salvar_erro: () => 'projeto_salvar_erro',
  criar_motores_erro: () => 'motores_erro', servidor_nao_existe: () => 'servidor_nao_existe',
  arquivo_carregando: () => 'carregando', arquivo_carregar_raizes_erro: () => 'raizes_erro',
  arquivo_sem_raizes: () => 'sem_raizes', arquivo_buscar_pasta: () => 'buscar_pasta',
  arquivo_sem_subpastas: () => 'sem_subpastas', arquivo_sem_resultados: () => 'sem_resultados',
  arquivo_usar_pasta: () => 'usar_pasta', arquivo_abrir: ({ nome }: { nome: string }) => `abrir:${nome}`,
  arquivo_ler_falhou: () => 'ler_falhou', arquivo_pasta_nao_encontrada: () => 'pasta_nao_encontrada',
  arquivo_sem_permissao: () => 'sem_permissao', arquivo_ilegivel: () => 'ilegivel',
  arquivo_raiz_nao_liberada: () => 'raiz_nao_liberada', arquivo_caminho_invalido: () => 'caminho_invalido',
  native_create_provider_missing: ({ p }: { p: string }) => `native_create_provider_missing:${p}`,
  native_create_provider_execution_denied: ({ p }: { p: string }) => `native_create_provider_execution_denied:${p}`,
  native_create_roots: () => 'native_create_roots', native_create_roots_failed: () => 'native_create_roots_failed',
  native_create_search: () => 'native_create_search', native_create_path: () => 'native_create_path',
  native_create_use_folder: () => 'native_create_use_folder', native_create_computer_folder: () => 'native_create_computer_folder',
  native_create_no_subfolders: () => 'native_create_no_subfolders', native_create_no_results: () => 'native_create_no_results',
  native_create_open: ({ nome }: { nome: string }) => `native_create_open:${nome}`, native_create_use: () => 'native_create_use',
  native_create_path_placeholder: () => 'native_create_path_placeholder', native_create_path_aria: () => 'native_create_path_aria',
  native_retry: () => 'native_retry', cota_conta_no_limite: ({ janela }: { janela: string }) => `cota_conta_no_limite:${janela}`,
  new_chat_account_switched: ({ conta }: { conta: string }) => `new_chat_account_switched:${conta}`,
  native_dictation_active: () => 'native_dictation_active', native_dictation_level: () => 'native_dictation_level',
  native_dictation_cancel: () => 'native_dictation_cancel',
  nova_conversa_placeholder: () => 'nova_conversa_placeholder', nova_conversa_enviar: () => 'nova_conversa_enviar',
  native_new_chat_folder: () => 'native_new_chat_folder', native_empty_chat_hint: () => 'native_empty_chat_hint',
  nova_conversa_opcoes: () => 'nova_conversa_opcoes', nova_conversa_sem_destino: () => 'nova_conversa_sem_destino',
  nova_conversa_destino_hint: () => 'nova_conversa_destino_hint', nova_conversa_config_hint: () => 'nova_conversa_config_hint',
  nova_conversa_guardada: () => 'nova_conversa_guardada', nova_conversa_abrir: () => 'nova_conversa_abrir',
  nova_conversa_reenviar: () => 'nova_conversa_reenviar', nova_conversa_conferir: () => 'nova_conversa_conferir',
  new_conversation_received_messages: () => 'new_conversation_received_messages',
  new_conversation_no_received_messages: () => 'new_conversation_no_received_messages',
  nova_conversa_adotar: () => 'nova_conversa_adotar', nova_conversa_descartar: () => 'nova_conversa_descartar',
  nova_conversa_nome_automatico: () => 'nova_conversa_nome_automatico',
  nova_conversa_retomada_selecionada: () => 'nova_conversa_retomada_selecionada',
  nova_conversa_criacao_incerta: () => 'nova_conversa_criacao_incerta', nova_conversa_envio_incerto: () => 'nova_conversa_envio_incerto',
  nova_conversa_salvar_erro: () => 'nova_conversa_salvar_erro', nova_conversa_servidor_ausente: () => 'nova_conversa_servidor_ausente',
  nova_conversa_resultado_salvar_erro: () => 'nova_conversa_resultado_salvar_erro',
  nova_conversa_envio_recusado: ({ erro }: { erro: string }) => `nova_conversa_envio_recusado:${erro}`,
  nova_conversa_envio_conferir_erro: ({ erro }: { erro: string }) => `nova_conversa_envio_conferir_erro:${erro}`,
  // Chaves das peças do redesign (caixa de envio, anexo, ditado, uso, menu do chip): devolvem o próprio nome.
  ...Object.fromEntries((
    'board_arquivo board_falha_upload board_imagem board_remover_anexo chat_envio_incerto chat_historico_sem_resposta chat_nao_carregou_historico ' +
    'chat_servidor_removido composer_adicionar_ao_chat composer_adicionar_arquivos composer_anexar_arquivo composer_camera composer_ditado_indisponivel ' +
    'composer_ditado_interrompido composer_falha_gravacao composer_falha_transcricao composer_fotos composer_gravar_audio composer_mensagem_para ' +
    'composer_mic_style_hint composer_parar_gravacao composer_sem_acesso_camera composer_sem_acesso_fotos composer_sem_acesso_mic ' +
    'composer_submission_recover_first composer_transcrevendo_audio composer_transcrever_de_novo composer_transcricao_vazia criar_mais_opcoes ' +
    'criar_modo_exec criar_modo_exec_headless criar_modo_exec_headless_resumo criar_modo_exec_headless_resumo_codex criar_modo_exec_tmux ' +
    'criar_modo_exec_tmux_resumo criar_modo_exec_tmux_resumo_codex ctx_anexos_da_sessao ditado_estilo_titulo draft_clear_error draft_invalid ' +
    'draft_read_error draft_write_error home_usage_30d home_usage_7d home_usage_active_days home_usage_activity home_usage_all home_usage_cost ' +
    'home_usage_day home_usage_empty home_usage_load_failed home_usage_method home_usage_model_count home_usage_models home_usage_overview ' +
    'home_usage_partial home_usage_period_unsupported home_usage_sessions home_usage_today home_usage_tokens home_usage_top_model home_usage_warming ' +
    'home_usage_warming_timeout native_close native_create_checkout_branch native_create_checkout_current native_create_checkout_failed ' +
    'native_create_checkout_loading native_create_checkout_worktree ' +
    'native_create_codex_account native_loading native_new_chat_machine ' +
    'native_new_chat_no_accounts nova_conversa_anexo_falhou nova_conversa_candidata nova_conversa_conferir_erro nova_conversa_criando ' +
    'nova_conversa_nao_encontrada nova_conversa_nome_conflito nova_conversa_so_terminal nova_conversa_tentativa_invalida sync_retry ' +
    'worktree_base worktree_modo worktree_modo_ajuda worktree_nome_branch worktree_nova_branch ' +
    'composer_session_settings composer_session_settings_hint native_ctl_model_title native_ctl_search native_ctl_no_results ' +
    'comum_nenhum_modelo native_new_chat_reasoning modelo_sem_niveis native_ctl_loading native_ctl_retry native_ctl_failed ' +
    'native_ctl_fast native_ctl_fast_on native_ctl_fast_off native_ctl_fast_hint native_ctl_fast_unavailable ' +
    'reasoning_level_low reasoning_level_medium reasoning_level_high reasoning_level_xhigh reasoning_level_max reasoning_level_ultra ' +
    'native_mode_plan native_mode_manual native_mode_auto native_mode_acceptEdits native_mode_bypassPermissions native_mode_dontAsk chat_mode_plan config_motores_modelo_padrao ' +
    'codex_contexto_titulo codex_contexto_padrao codex_contexto_salvando comum_falha_aplicar config_server_tentar_de_novo home_new_conversation'
  ).split(' ').map((k) => [k, () => k])),
}));

import { motivoCotaAntiga, type CotaContaResumo } from '@hangar/core';
import { CreateSessionSheet } from './CreateSessionSheet';
import { _resetNewConversationForTests } from '../../stores/newConversation';
import { _resetCarriedForTests } from './NewConversation';

const connected = { id: 'work', credential_id: 'codex:/work', name: 'Trabalho', home: '/work', is_default: false, auth: { method: 'oauth', status: 'connected', email: 'work@example.com', plan: 'Plus' }, sync: { status: 'ready', trust_pending: false, issues: [] } };
const defaultAccount = { id: 'default', credential_id: 'codex:/default', name: 'Padrão', home: '/default', is_default: true, auth: { method: 'oauth', status: 'connected', email: 'default@example.com', plan: 'Plus' }, sync: { status: 'ready', trust_pending: false, issues: [] } };
const archived = { project: '-repo', cwd: '/repo', session_id: 'sid-1', mtime: 1, preview: 'continuação', ultima: 'última mensagem', live: false, config_dir: null, conta: 'Trabalho', provider: 'codex', codex_account: 'work', codex_home: '/work' };

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
}

describe('CreateSessionSheet Codex', () => {
  afterEach(() => { vi.useRealTimers(); vi.restoreAllMocks(); });
  beforeEach(() => {
    calls.contextFetch.mockReset().mockImplementation(async () => new Response(JSON.stringify({ contexto_estendido: true })));
    vi.spyOn(globalThis, 'fetch').mockImplementation((input, init) => calls.contextFetch(input, init));
    calls.dismiss = null;
    calls.pickerActive = 0;
    calls.pickerPeak = 0;
    calls.pickerHistory = [];
    calls.target = null;
    calls.activeId = server.id;
    calls.startPending = null;
    calls.cotas = [];
    calls.save.mockReset();
    calls.startDictation.mockReset();
    calls.stopDictation.mockReset();
    calls.transcribe.mockReset().mockResolvedValue({ text: 'spoken-fixture' });
    calls.roots.mockReset().mockResolvedValue([{ name: 'Repo', path: '/repo' }]);
    calls.scan.mockReset().mockResolvedValue({ entries: [] });
    calls.sessions.mockReset().mockResolvedValue([]);
    calls.configs.mockReset().mockResolvedValue([]);
    calls.engines.mockReset().mockResolvedValue({ motores: {} });
    calls.accounts.mockReset().mockResolvedValue([connected]);
    calls.mutate.mockReset();
    calls.providers.mockReset().mockResolvedValue({});
    calls.models.mockReset().mockResolvedValue({ models: [], reduced: false });
    calls.prepare.mockReset().mockResolvedValue({ status: 'ready', trust_pending: false, issues: [] });
    calls.preparation.mockReset();
    calls.create.mockReset().mockResolvedValue({ name: 'nova', state: 'idle' });
    calls.send.mockReset().mockResolvedValue(undefined);
    calls.history.mockReset().mockResolvedValue([]);
    calls.archives.mockReset().mockResolvedValue([]);
    calls.resume.mockReset().mockResolvedValue({ name: 'retomada', state: 'idle' });
    calls.replace.mockReset();
    calls.alert.mockReset();
    localStorage.clear();
    _resetNewConversationForTests();
    // O texto digitado sobrevive à remontagem da tela; entre testes ele não pode vazar.
    _resetCarriedForTests();
  });

  const button = (container: HTMLElement, text: string) => {
    const buttons = [...container.querySelectorAll('button')];
    return buttons.find((b) => b.getAttribute('aria-label') === text) ?? buttons.find((b) => b.textContent === text);
  };

  function type(container: HTMLElement, text: string) {
    const input = container.querySelector('textarea[aria-label="nova_conversa_placeholder"]') as HTMLTextAreaElement;
    input.value = text;
    input.dispatchEvent(new Event('input', { bubbles: true }));
  }

  async function send(container: HTMLElement, text = 'oi') {
    for (const panel of container.querySelectorAll<HTMLElement>('[data-testid="folder-panel"][data-open="true"]')) {
      await act(async () => button(panel, 'close-panel')!.click());
    }
    if (calls.dismiss) await flushDismiss();
    await act(async () => type(container, text));
    await act(async () => button(container, 'nova_conversa_enviar')!.click());
    for (let i = 0; i < 4; i++) await act(async () => Promise.resolve());
  }

  async function openSettings(container: HTMLElement) {
    if (!container.querySelector('[data-testid="folder-panel"][aria-label="composer_session_settings"]')) {
      await act(async () => button(container, 'composer_session_settings')!.click());
    }
  }

  async function closePanel(container: HTMLElement, label?: string) {
    const panel = label ? container.querySelector(`[data-testid="folder-panel"][aria-label="${label}"]`) : container;
    await act(async () => button(panel as HTMLElement, 'close-panel')!.click());
  }

  async function openOptions(container: HTMLElement) {
    await openSettings(container);
    const more = button(container, 'criar_mais_opcoes')!;
    if (more.getAttribute('aria-expanded') !== 'true') await act(async () => more.click());
  }

  async function openFolder(container: HTMLElement) {
    const trigger = [...container.querySelectorAll<HTMLButtonElement>('button')].find((b) => b.getAttribute('aria-label')?.startsWith('native_new_chat_folder:'))!;
    await act(async () => trigger.click());
  }

  async function pickAccount(container: HTMLElement, value: string, codex = true) {
    const label = codex ? 'native_create_codex_account' : 'comum_conta_claude';
    const trigger = [...container.querySelectorAll<HTMLButtonElement>('button')].find((b) => b.getAttribute('aria-label')?.startsWith(`${label}:`))!;
    await act(async () => trigger.click());
    await act(async () => button(container, value)!.click());
  }

  async function flushDismiss() {
    expect(calls.dismiss).toBeTruthy();
    await act(async () => calls.dismiss!());
  }

  async function renderSheet(strict = false, chooseCodex = true) {
    const container = document.createElement('div');
    const root = createRoot(container);
    await act(async () => root.render(strict ? createElement(StrictMode, null, createElement(CreateSessionSheet)) : createElement(CreateSessionSheet)));
    await act(async () => Promise.resolve());
    if (chooseCodex) {
      await openSettings(container);
      await act(async () => button(container, 'Codex')!.click());
      await closePanel(container, 'composer_session_settings');
    }
    await act(async () => Promise.resolve());
    return { container, root };
  }

  it('home mantém campo, anexo e destino montados entre lista e criação, e uso durante a escrita', async () => {
    const container = document.createElement('div');
    const root = createRoot(container);
    let focusRequest = 0;
    const show = async (mode: 'sessions' | 'newConversation') => act(async () => root.render(createElement(CreateSessionSheet, {
      home: { mode, focusRequest, onNewConversation: vi.fn(), sessions: createElement('div', { 'data-home-list': true }) },
    })));
    await show('sessions');
    expect(button(container, 'home_new_conversation')).toBeTruthy();
    const input = container.querySelector<HTMLTextAreaElement>('textarea[aria-label="nova_conversa_placeholder"]')!;
    const focus = vi.spyOn(input, 'focus');
    expect(focus).not.toHaveBeenCalled();
    focusRequest++;
    await show('newConversation');
    expect(focus).toHaveBeenCalledTimes(1);
    await act(async () => type(container, 'kept-draft'));
    expect(container.querySelector('[data-home-usage]')).not.toBeNull();
    await act(async () => button(container, 'composer_adicionar_ao_chat')!.click());
    await act(async () => button(container, 'pick-fixture')!.click());
    expect(container.textContent).toContain('fixture.png');
    const folder = button(container, 'native_new_chat_folder: repo');
    await show('sessions');
    expect(container.querySelector('textarea[aria-label="nova_conversa_placeholder"]')).toBe(input);
    focusRequest++;
    await show('newConversation');
    expect(input.value).toBe('kept-draft');
    expect(container.textContent).toContain('fixture.png');
    expect(button(container, 'native_new_chat_folder: repo')).toBe(folder);
    expect(calls.create).not.toHaveBeenCalled();
    expect(calls.send).not.toHaveBeenCalled();
    await act(async () => root.unmount());
  });

  it('home inicia ditado pela faixa sem pedir teclado e transcreve ao voltar à lista', async () => {
    const container = document.createElement('div');
    const root = createRoot(container);
    const onNewConversation = vi.fn(() => render('newConversation'));
    const render = (mode: 'sessions' | 'newConversation') => root.render(createElement(CreateSessionSheet, {
      home: { mode, focusRequest: 0, onNewConversation, sessions: createElement('div', { 'data-home-list': true }) },
    }));
    const show = async (mode: 'sessions' | 'newConversation') => act(async () => render(mode));
    await show('sessions');
    expect(button(container, 'home_new_conversation')).toBeTruthy();
    const input = container.querySelector<HTMLTextAreaElement>('textarea[aria-label="nova_conversa_placeholder"]')!;
    const focus = vi.spyOn(input, 'focus');
    const mic = container.querySelector<HTMLButtonElement>('button[aria-label="composer_gravar_audio"]')!;
    await act(async () => mic.click());
    expect(onNewConversation).toHaveBeenCalledWith(false);
    expect(calls.startDictation).toHaveBeenCalledTimes(1);
    expect(calls.stopDictation).not.toHaveBeenCalled();
    expect(focus).not.toHaveBeenCalled();
    await show('sessions');
    expect(calls.stopDictation).toHaveBeenCalledTimes(1);
    expect(calls.transcribe).toHaveBeenCalledTimes(1);
    await show('newConversation');
    expect(input.value).toBe('spoken-fixture');
    await act(async () => root.unmount());
  });

  it('home conserva a máquina e recebe transcrição em voo quando o chat troca activeId', async () => {
    calls.target = { id: 'server-c', label: 'Servidor C', baseUrl: 'https://c.local', token: 'token-c' };
    const transcription = deferred<{ text: string }>();
    calls.transcribe.mockReturnValueOnce(transcription.promise);
    const container = document.createElement('div');
    const root = createRoot(container);
    const home = { mode: 'newConversation' as const, focusRequest: 0, onNewConversation: vi.fn(), sessions: null };
    await act(async () => root.render(createElement(CreateSessionSheet, { home })));
    const input = container.querySelector<HTMLTextAreaElement>('textarea[aria-label="nova_conversa_placeholder"]')!;
    await act(async () => button(container, 'composer_gravar_audio')!.click());
    await act(async () => button(container, 'composer_parar_gravacao')!.click());
    expect(calls.transcribe).toHaveBeenCalledTimes(1);
    calls.activeId = 'server-c';
    await act(async () => root.render(createElement(CreateSessionSheet, { home })));
    expect(container.querySelector('textarea[aria-label="nova_conversa_placeholder"]')).toBe(input);
    await act(async () => transcription.resolve({ text: 'kept-transcription' }));
    expect(input.value).toBe('kept-transcription');
    expect(button(container, 'native_new_chat_machine: Servidor B')).toBeTruthy();
    await act(async () => root.unmount());
  });

  it('home cancela pedido de foco pendente na transcrição antes da próxima abertura por mic', async () => {
    const transcription = deferred<{ text: string }>();
    calls.transcribe.mockReturnValueOnce(transcription.promise);
    const container = document.createElement('div');
    const root = createRoot(container);
    let focusRequest = 0;
    const onNewConversation = vi.fn((focus: boolean) => {
      if (focus) focusRequest++;
      render('newConversation');
    });
    const render = (mode: 'sessions' | 'newConversation'): void => root.render(createElement(CreateSessionSheet, {
      home: { mode, focusRequest, onNewConversation, sessions: null },
    }));
    const show = async (mode: 'sessions' | 'newConversation') => act(async () => render(mode));
    await show('newConversation');
    const input = container.querySelector<HTMLTextAreaElement>('textarea[aria-label="nova_conversa_placeholder"]')!;
    const focus = vi.spyOn(input, 'focus');
    await act(async () => button(container, 'composer_gravar_audio')!.click());
    await act(async () => button(container, 'composer_parar_gravacao')!.click());
    await show('sessions');
    await act(async () => button(container, 'home_new_conversation')!.click());
    expect(focus).not.toHaveBeenCalled();
    await show('sessions');
    await act(async () => transcription.resolve({ text: 'spoken-fixture' }));
    const permission = deferred<void>();
    calls.startPending = permission.promise;
    await act(async () => button(container, 'composer_gravar_audio')!.click());
    expect(calls.startDictation).toHaveBeenCalledTimes(2);
    expect(focus).not.toHaveBeenCalled();
    await act(async () => permission.resolve());
    await act(async () => root.unmount());
  });

  it('abre a máquina em painel ancorado e não cria sessão ao trocar o destino', async () => {
    calls.target = { id: 'server-c', label: 'Servidor C', baseUrl: 'https://c.local', token: 'token-c' };
    const { container, root } = await renderSheet(false, false);
    expect(container.querySelector('[aria-label="native_new_chat_machine"]')).toBeNull();
    await act(async () => type(container, 'rascunho entre máquinas'));
    await act(async () => button(container, 'native_new_chat_machine: Servidor B')!.click());
    expect(container.querySelector('[data-testid="folder-panel"][aria-label="native_new_chat_machine"]')).not.toBeNull();
    await act(async () => button(container, 'Servidor C')!.click());
    expect(button(container, 'native_new_chat_machine: Servidor C')).toBeTruthy();
    expect(container.querySelector('[data-testid="folder-panel"]')).toBeNull();
    expect(calls.create).not.toHaveBeenCalled();
    expect(calls.send).not.toHaveBeenCalled();
    expect((container.querySelector('textarea[aria-label="nova_conversa_placeholder"]') as HTMLTextAreaElement).value).toBe('rascunho entre máquinas');
    await act(async () => button(container, 'nova_conversa_enviar')!.click());
    for (let i = 0; i < 4; i++) await act(async () => Promise.resolve());
    expect(calls.create).toHaveBeenCalledExactlyOnceWith(calls.target, expect.objectContaining({ cwd: '/repo' }));
    expect(calls.send).toHaveBeenCalledWith(calls.target, 'nova', 'rascunho entre máquinas');
    root.unmount();
  });

  it.each([
    { defaultModel: 'sol', alias: false, active: false, available: true },
    { defaultModel: undefined, alias: false, active: true, available: false },
    { defaultModel: undefined, alias: false, active: false, available: false },
    { defaultModel: undefined, alias: true, active: false, available: false },
  ])('Fast fixa somente padrão Codex identificado: $defaultModel / alias=$alias / active=$active', async ({ defaultModel, alias, active, available }) => {
    calls.providers.mockResolvedValue({ codex: { disponivel: true, default: true } });
    calls.models.mockResolvedValue({ kind: 'codex', reduced: false, default: defaultModel, models: [
      { id: alias ? 'default' : 'sol', name: 'Sol', active, context_length: 272000,
        efforts: ['low', 'high'], default_effort: 'low', service_tiers: [{ id: 'priority', hidden: false }] },
    ] });
    const { container, root } = await renderSheet(false, false);
    await openSettings(container);
    const fast = container.querySelector<HTMLButtonElement>('[role="switch"][aria-label="native_ctl_fast"]')!;
    expect(fast.disabled).toBe(!available);
    await act(async () => fast.click());
    expect(calls.create).not.toHaveBeenCalled();
    expect(calls.mutate).not.toHaveBeenCalled();
    await send(container);
    if (available) expect(calls.create.mock.calls[0][1]).toMatchObject({ model: 'sol', service_tier: 'priority' });
    else {
      expect(calls.create.mock.calls[0][1].model).toBeNull();
      expect(calls.create.mock.calls[0][1]).not.toHaveProperty('service_tier');
    }
    root.unmount();
  });

  it('contas Claude tardias não recarregam nem apagam escolhas do catálogo Codex', async () => {
    const configs = deferred<{ path: string; label: string; active: boolean }[]>();
    calls.configs.mockReturnValue(configs.promise);
    calls.providers.mockResolvedValue({ codex: { disponivel: true, default: true } });
    calls.models.mockResolvedValue({ kind: 'codex', reduced: false, models: [
      { id: 'sol', name: 'Sol', efforts: ['low', 'high'], default_effort: 'low', service_tiers: [{ id: 'priority' }] },
    ] });
    const { container, root } = await renderSheet(false, false);
    await openSettings(container);
    await act(async () => button(container, 'Sol')!.click());
    await act(async () => button(container, 'reasoning_level_high')!.click());
    await act(async () => container.querySelector<HTMLButtonElement>('[role="switch"][aria-label="native_ctl_fast"]')!.click());
    const codexReads = calls.models.mock.calls.filter((call) => call[1] === 'codex').length;
    await act(async () => configs.resolve([{ path: '/late-claude', label: 'Conta tardia', active: true }]));
    expect(calls.models.mock.calls.filter((call) => call[1] === 'codex')).toHaveLength(codexReads);
    expect(container.querySelector('[role="switch"][aria-label="native_ctl_fast"]')!.getAttribute('aria-checked')).toBe('true');
    expect(button(container, 'composer_session_settings')!.textContent).toContain('Sol high');
    await send(container);
    expect(calls.create).toHaveBeenCalledExactlyOnceWith(server, expect.objectContaining({
      provider: 'codex', model: 'sol', effort: 'high', service_tier: 'priority', codex_account: 'work',
    }));
    root.unmount();
  });

  it.each([
    { closing: 'recolher', failure: false }, { closing: 'fechar', failure: false },
    { closing: 'recolher', failure: true }, { closing: 'fechar', failure: true },
  ])('contexto global termina a escrita após $closing extras, com erro=$failure', async ({ closing, failure }) => {
    const post = deferred<Response>();
    calls.contextFetch.mockImplementation(async (_url, init) => init?.method === 'POST'
      ? post.promise : new Response(JSON.stringify({ contexto_estendido: true })));
    const { container, root } = await renderSheet();
    try {
      await openOptions(container);
      const contextSwitch = () => container.querySelector<HTMLButtonElement>('[role="switch"][aria-label="codex_contexto_titulo"]')!;
      expect(contextSwitch().getAttribute('aria-checked')).toBe('true');
      await act(async () => contextSwitch().click());
      const write = calls.contextFetch.mock.calls.find((call) => call[1]?.method === 'POST')!;
      expect(write[0]).toBe('https://b.local/api/harness/codex/opcoes');
      expect(write[1].body).toBe('{"contexto_estendido":false}');
      if (closing === 'recolher') await act(async () => button(container, 'criar_mais_opcoes')!.click());
      else await closePanel(container, 'composer_session_settings');
      expect((write[1].signal as AbortSignal).aborted).toBe(false);
      await act(async () => type(container, 'mensagem aguardando contexto'));
      expect(button(container, 'nova_conversa_enviar')!.disabled).toBe(true);
      await act(async () => post.resolve(failure
        ? new Response(JSON.stringify({ detail: 'context-write-failed' }), { status: 500 })
        : new Response(JSON.stringify({ contexto_estendido: false }))));
      expect(button(container, 'nova_conversa_enviar')!.disabled).toBe(false);
      if (failure) expect(container.textContent).toContain('context-write-failed');
      await openOptions(container);
      expect(contextSwitch().getAttribute('aria-checked')).toBe(String(failure));
      if (failure) expect(container.textContent).toContain('context-write-failed');
      expect(calls.contextFetch.mock.calls.filter((call) => call[1]?.method === 'GET')).toHaveLength(1);
      expect(calls.create).not.toHaveBeenCalled();
    } finally { await act(async () => root.unmount()); }
  });

  it('fechar contexto cancela só GET e uma leitura antiga não vence a reabertura', async () => {
    const oldRead = deferred<Response>();
    calls.contextFetch.mockReturnValueOnce(oldRead.promise)
      .mockImplementation(async () => new Response(JSON.stringify({ contexto_estendido: false })));
    const { container, root } = await renderSheet();
    try {
      await openOptions(container);
      const readSignal = calls.contextFetch.mock.calls[0][1].signal as AbortSignal;
      await closePanel(container, 'composer_session_settings');
      expect(readSignal.aborted).toBe(true);
      await openOptions(container);
      await act(async () => oldRead.resolve(new Response(JSON.stringify({ contexto_estendido: true }))));
      expect(container.querySelector('[role="switch"][aria-label="codex_contexto_titulo"]')!.getAttribute('aria-checked')).toBe('false');
      expect(calls.contextFetch.mock.calls.every((call) => call[1]?.method === 'GET')).toBe(true);
    } finally { await act(async () => root.unmount()); }
  });

  it('voltar à linha Padrão após Fast limpa tier e conserva model null no envio', async () => {
    calls.providers.mockResolvedValue({ codex: { disponivel: true, default: true } });
    calls.models.mockResolvedValue({ kind: 'codex', reduced: false, default: 'sol', models: [
      { id: 'sol', name: 'Sol', efforts: ['high'], service_tiers: [{ id: 'priority' }] },
    ] });
    const { container, root } = await renderSheet(false, false);
    await openSettings(container);
    await act(async () => container.querySelector<HTMLButtonElement>('[role="switch"][aria-label="native_ctl_fast"]')!.click());
    await act(async () => button(container, 'criar_padrao')!.click());
    await send(container);
    expect(calls.create.mock.calls[0][1].model).toBeNull();
    expect(calls.create.mock.calls[0][1]).not.toHaveProperty('service_tier');
    root.unmount();
  });

  it.each([false, true])('desligar Fast omite service_tier sem desfazer o modelo escolhido (mesmo lote: %s)', async (batched) => {
    calls.providers.mockResolvedValue({ codex: { disponivel: true, default: true } });
    calls.models.mockResolvedValue({ kind: 'codex', reduced: false, models: [
      { id: 'sol', name: 'Sol', efforts: ['high'], service_tiers: [{ id: 'priority' }] },
    ] });
    const { container, root } = await renderSheet(false, false);
    await openSettings(container);
    await act(async () => button(container, 'Sol')!.click());
    const fast = () => container.querySelector<HTMLButtonElement>('[role="switch"][aria-label="native_ctl_fast"]')!;
    if (batched) await act(async () => { fast().click(); fast().click(); });
    else {
      await act(async () => fast().click());
      expect(fast().getAttribute('aria-checked')).toBe('true');
      await act(async () => fast().click());
    }
    expect(fast().getAttribute('aria-checked')).toBe('false');
    await send(container);
    expect(calls.create.mock.calls[0][1].model).toBe('sol');
    expect(calls.create.mock.calls[0][1]).not.toHaveProperty('service_tier');
    root.unmount();
  });

  it('pílula abre modelos, provider e esforço reais; escolhas só saem ao Enviar', async () => {
    calls.models.mockResolvedValue({ kind: 'codex', reduced: false, models: [
      { id: 'sol', name: 'Sol', context_length: 272000, efforts: ['low', 'high'], default_effort: 'low', service_tiers: [{ id: 'priority', hidden: false }] },
      { id: 'mini', name: 'Mini', context: '128K', efforts: ['low'], default_effort: 'low', service_tiers: [] },
    ] });
    const { container, root } = await renderSheet(false, false);
    expect(button(container, 'Sol')).toBeUndefined();
    const chip = button(container, 'composer_session_settings');
    expect(chip).toBeTruthy();
    await act(async () => chip!.click());
    await act(async () => button(container, 'Codex')!.click());
    await act(async () => button(container, 'Sol')!.click());
    await act(async () => button(container, 'reasoning_level_high')!.click());
    const fast = container.querySelector<HTMLButtonElement>('[role="switch"][aria-label="native_ctl_fast"]');
    expect(fast).not.toBeNull();
    await act(async () => fast!.click());
    expect(button(container, 'composer_session_settings')!.textContent).toBe('Sol · 272K high · native_ctl_fast');
    expect(calls.create).not.toHaveBeenCalled();
    expect(calls.send).not.toHaveBeenCalled();
    expect(calls.mutate).not.toHaveBeenCalled();
    await act(async () => button(container, 'close-panel')!.click());
    expect(button(container, 'Sol')).toBeUndefined();
    await send(container);
    expect(calls.create).toHaveBeenCalledExactlyOnceWith(server, expect.objectContaining({
      provider: 'codex', model: 'sol', effort: 'high', service_tier: 'priority', codex_account: 'work',
    }));
    root.unmount();
  });

  it.each([
    { tiers: [{ id: 'priority' }], available: true },
    { tiers: [{ id: 'priority', hidden: true }], available: false },
    { tiers: [], available: false },
  ])('Fast na criação usa apenas o modelo selecionado: $tiers', async ({ tiers, available }) => {
    calls.providers.mockResolvedValue({ codex: { disponivel: true, default: true } });
    calls.models.mockResolvedValue({ kind: 'codex', reduced: false, models: [
      { id: 'sol', name: 'Sol', efforts: ['high'], service_tiers: tiers },
      { id: 'other', name: 'Other', efforts: ['low'], service_tiers: [{ id: 'priority' }] },
    ] });
    const { container, root } = await renderSheet(false, false);
    expect(button(container, 'composer_session_settings')).toBeTruthy();
    await act(async () => button(container, 'composer_session_settings')!.click());
    await act(async () => button(container, 'Sol')!.click());
    const fast = container.querySelector<HTMLButtonElement>('[role="switch"][aria-label="native_ctl_fast"]');
    expect(fast).not.toBeNull();
    expect(fast!.disabled).toBe(!available);
    await act(async () => fast!.click());
    expect(calls.mutate).not.toHaveBeenCalled();
    await send(container);
    const payload = calls.create.mock.calls[0][1];
    if (available) expect(payload.service_tier).toBe('priority');
    else expect(payload).not.toHaveProperty('service_tier');
    root.unmount();
  });

  it('toque longo no microfone da criação abre o estilo sem criar nem enviar', async () => {
    const { container, root } = await renderSheet(false, false);
    const mic = button(container, 'composer_gravar_audio')!;
    await act(async () => mic.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true })));
    expect(container.querySelector('[role="dialog"][aria-label="ditado_estilo_titulo"]')).not.toBeNull();
    expect(calls.create).not.toHaveBeenCalled();
    expect(calls.send).not.toHaveBeenCalled();
    root.unmount();
  });

  it('Mais opções expande no mesmo painel ancorado, sem trocar o modal do modelo', async () => {
    const { container, root } = await renderSheet(false, false);
    await openSettings(container);
    const panel = container.querySelector('[data-testid="folder-panel"][aria-label="composer_session_settings"]');
    await act(async () => button(container, 'criar_mais_opcoes')!.click());
    expect(container.querySelector('[data-testid="folder-panel"][aria-label="composer_session_settings"]')).toBe(panel);
    expect(container.querySelectorAll('[data-testid="folder-panel"]')).toHaveLength(1);
    expect(container.querySelector('[data-testid="options-sheet"]')).toBeNull();
    expect(container.querySelector('textarea[aria-label="comum_nome"]')).not.toBeNull();
    await act(async () => button(container, 'criar_mais_opcoes')!.click());
    expect(container.querySelector('textarea[aria-label="comum_nome"]')).toBeNull();
    expect(container.querySelector('[data-testid="folder-panel"][aria-label="composer_session_settings"]')).toBe(panel);
    expect(calls.create).not.toHaveBeenCalled();
    expect(calls.mutate).not.toHaveBeenCalled();
    root.unmount();
  });

  it('extras do mesmo painel guardam nome, permissão e subagente só para o Enviar', async () => {
    calls.models.mockResolvedValue({ kind: 'claude', reduced: false, models: [
      { id: 'opus', name: 'Opus', context_length: 1000000 }, { id: 'sonnet', name: 'Sonnet', context_length: 200000 },
    ] });
    const { container, root } = await renderSheet(false, false);
    await openSettings(container);
    await act(async () => button(container, 'criar_mais_opcoes')!.click());
    expect(container.querySelector('[data-testid="folder-panel"][aria-label="composer_session_settings"]')).not.toBeNull();
    await act(async () => {
      const field = container.querySelector('textarea[aria-label="comum_nome"]') as HTMLTextAreaElement;
      field.value = 'conversa-local';
      field.dispatchEvent(new Event('input', { bubbles: true }));
    });
    await act(async () => button(container, 'criar_permissao_padrao')!.click());
    await act(async () => button(container, 'bypassPermissions')!.click());
    await act(async () => button(container, 'criar_subagente_padrao')!.click());
    const subagents = container.querySelector<HTMLElement>('[data-testid="folder-panel"][aria-label="criar_subagente_padrao"]')!;
    await act(async () => button(subagents, 'Sonnet')!.click());
    expect(calls.create).not.toHaveBeenCalled();
    expect(calls.mutate).not.toHaveBeenCalled();
    await send(container);
    expect(calls.create).toHaveBeenCalledExactlyOnceWith(server, expect.objectContaining({
      name: 'conversa-local', permission_mode: 'bypassPermissions', subagent_model: 'sonnet', model: null,
    }));
    root.unmount();
  });

  it('mantém indicador visível no Enviar enquanto a criação espera resposta', async () => {
    calls.create.mockReturnValue(new Promise(() => {}));
    const { container, root } = await renderSheet(false, false);
    await send(container);
    expect(button(container, 'nova_conversa_enviar')!.textContent).toBe('loading');
    root.unmount();
  });

  it.each([
    { provider: 'claude', motivo: 'nao_encontrado', reason: 'native_create_provider_missing:OMP' },
    { provider: 'codex', motivo: 'nao_encontrado', reason: 'native_create_provider_missing:OMP' },
    { provider: 'claude', motivo: 'sem_permissao', reason: 'native_create_provider_execution_denied:OMP' },
    { provider: 'codex', motivo: 'sem_permissao', reason: 'native_create_provider_execution_denied:OMP' },
    { provider: 'claude', motivo: null, reason: 'native_create_provider_missing:OMP' },
    { provider: 'codex', motivo: null, reason: 'native_create_provider_missing:OMP' },
  ])('traduz motivo da API para OMP indisponível com $provider ativo: $motivo', async ({ provider, motivo, reason }) => {
    calls.providers.mockResolvedValue({
      claude: { disponivel: true, motivo: null, default: provider === 'claude' },
      codex: { disponivel: true, motivo: null, default: provider === 'codex' },
      omp: { disponivel: false, motivo, default: false },
    });
    const { container, root } = await renderSheet(false, false);
    try {
      await openSettings(container);
      expect(container.textContent).toContain(reason);
      expect(container.textContent).not.toContain('nao_encontrado');
      expect(container.textContent).not.toContain('sem_permissao');
      expect(button(container, 'OMP')!.disabled).toBe(true);
      expect(button(container, provider === 'claude' ? 'Claude' : 'Codex')!.disabled).toBe(false);
      await act(async () => button(container, 'OMP')!.click());
      await send(container);
      expect(calls.create).toHaveBeenCalledWith(server, expect.objectContaining({ provider }));
    } finally { await act(async () => root.unmount()); }
  });

  it('mostra o motivo do provider indisponível sem permitir escolhê-lo', async () => {
    calls.providers.mockResolvedValue({ claude: { disponivel: true }, kimi: { disponivel: false, motivo: 'Kimi não instalado nesta máquina' } });
    const { container, root } = await renderSheet(false, false);
    await openSettings(container);
    expect(button(container, 'Kimi')!.disabled).toBe(true);
    expect(container.textContent).toContain('Kimi não instalado nesta máquina');
    await act(async () => button(container, 'Kimi')!.click());
    await send(container);
    expect(calls.create).toHaveBeenCalledWith(server, expect.objectContaining({ provider: 'claude' }));
    root.unmount();
  });

  it('descarta esforço guardado incompatível com Haiku', async () => {
    localStorage.setItem('create.choices.v1:server-b', JSON.stringify({ provider: 'claude', model: 'haiku', effort: 'high' }));
    calls.models.mockResolvedValue({ kind: 'claude', reduced: false, models: [{ id: 'haiku', name: 'Haiku' }] });
    const { container, root } = await renderSheet(false, false);
    await send(container);
    expect(calls.create.mock.calls[0][1].effort).toBeNull();
    root.unmount();
  });

  it('conta e terminal abrem painéis e não aplicam nada à sessão antes de Enviar', async () => {
    calls.configs.mockResolvedValue([{ path: '/a', label: 'Conta A', active: true }, { path: '/b', label: 'Conta B', active: false }]);
    const { container, root } = await renderSheet(false, false);
    expect(button(container, 'Conta B')).toBeUndefined();
    await pickAccount(container, 'Conta B', false);
    await act(async () => button(container, 'criar_modo_exec: criar_modo_exec_headless')!.click());
    expect(container.querySelector('[data-testid="folder-panel"][aria-label="criar_modo_exec"]')).not.toBeNull();
    await act(async () => button(container, 'criar_modo_exec_tmux')!.click());
    expect(calls.create).not.toHaveBeenCalled();
    expect(calls.mutate).not.toHaveBeenCalled();
    expect(calls.models).toHaveBeenLastCalledWith(server, 'claude', null, '/b', null, expect.any(AbortSignal));
    await send(container);
    expect(calls.create).toHaveBeenCalledExactlyOnceWith(server, expect.objectContaining({ config_dir: '/b', headless: false }));
    root.unmount();
  });

  it.each([
    { kind: 'engine', global: true, model: true, visible: true, available: true },
    { kind: 'engine', global: true, model: false, visible: true, available: false },
    { kind: 'engine', global: false, model: true, visible: false, available: false },
    { kind: 'claude', global: true, model: true, visible: false, available: false },
  ])('Fast Claude exige motor e metadata do catálogo: $kind / $global / $model', async ({ kind, global, model, visible, available }) => {
    calls.engines.mockResolvedValue({ motores: { local: { label: 'Motor local', model: 'sol' } } });
    calls.models.mockResolvedValue({ kind, reduced: false, supports_fast: global, models: [{ id: 'sol', name: 'Sol', supports_fast: model }] });
    const { container, root } = await renderSheet(false, false);
    await openOptions(container);
    await act(async () => button(container, 'criar_claude_sua_conta')!.click());
    await act(async () => button(container, 'Motor local')!.click());
    await closePanel(container, 'composer_session_settings');
    await openSettings(container);
    await act(async () => button(container, 'Sol')!.click());
    const fast = container.querySelector<HTMLButtonElement>('[role="switch"][aria-label="native_ctl_fast"]');
    expect(!!fast).toBe(visible);
    if (fast) { expect(fast.disabled).toBe(!available); await act(async () => fast.click()); }
    expect(calls.mutate).not.toHaveBeenCalled();
    await send(container);
    const payload = calls.create.mock.calls[0][1];
    expect(payload.engine).toBe('local');
    if (available) expect(payload.service_tier).toBe('priority');
    else expect(payload).not.toHaveProperty('service_tier');
    root.unmount();
  });

  it.each(['priority', 'default'])('Fast %s não vaza ao trocar para modelo sem suporte ou outro provider', async (tier) => {
    calls.providers.mockResolvedValue({ codex: { disponivel: true, default: true } });
    calls.models.mockResolvedValue({ kind: 'codex', reduced: false, models: [
      { id: 'sol', name: 'Sol', efforts: ['high'], service_tiers: [{ id: 'priority' }] },
      { id: 'mini', name: 'Mini', efforts: ['low'], service_tiers: [] },
    ] });
    const { container, root } = await renderSheet(false, false);
    await openSettings(container);
    await act(async () => button(container, 'Sol')!.click());
    const fast = () => container.querySelector<HTMLButtonElement>('[role="switch"][aria-label="native_ctl_fast"]')!;
    await act(async () => fast().click());
    if (tier === 'default') await act(async () => fast().click());
    await act(async () => button(container, 'Mini')!.click());
    expect(fast().disabled).toBe(true);
    expect(fast().getAttribute('aria-checked')).toBe('false');
    await act(async () => button(container, 'Claude')!.click());
    expect(fast()).toBeNull();
    await send(container);
    expect(calls.create.mock.calls[0][1]).not.toHaveProperty('service_tier');
    expect(calls.mutate).not.toHaveBeenCalled();
    root.unmount();
  });

  it.each([
    { provider: 'claude', models: [{ id: 'sonnet', name: 'Sonnet de outra sessão', active: true, context_length: 1000000 }], label: 'config_motores_modelo_padrao', catalogDefault: undefined },
    { provider: 'claude', models: [{ id: 'default', name: 'Default (recommended)', context_length: 200000 }], label: 'criar_padrao · 200K', catalogDefault: undefined },
    { provider: 'claude', models: [{ id: 'opus', name: 'Opus 5.5' }], label: 'config_motores_modelo_padrao', catalogDefault: undefined },
    { provider: 'kimi', models: [{ id: 'kimi-k2', name: 'Kimi K2', context_length: 256000 }], label: 'Kimi K2 · 256K', catalogDefault: 'kimi-k2' },
  ])('sem preferências o chip informa o modelo conhecido ou padrão honestamente: $provider / $models.0.id / $label', async ({ provider, models, label, catalogDefault }) => {
    calls.providers.mockResolvedValue({ [provider]: { disponivel: true, default: true } });
    calls.models.mockResolvedValue({ kind: provider, reduced: false, models, default: catalogDefault });
    const { container, root } = await renderSheet(false, false);
    expect(button(container, 'composer_session_settings')!.textContent).toBe(label);
    await send(container);
    expect(calls.create.mock.calls[0][1].model).toBeNull();
    expect(calls.create.mock.calls[0][1]).not.toHaveProperty('service_tier');
    root.unmount();
  });

  it('usa o padrão GPT do servidor e sua conta conectada sem selecionar Claude', async () => {
    calls.providers.mockResolvedValue({
      claude: { disponivel: true, motivo: null, default: false },
      codex: { disponivel: true, motivo: null, default: true },
    });
    calls.accounts.mockResolvedValue([
      { ...defaultAccount, auth: { ...defaultAccount.auth, status: 'disconnected' } }, connected,
    ]);
    const { container, root } = await renderSheet(false, false);
    await send(container);
    expect(calls.create).toHaveBeenCalledWith(server, expect.objectContaining({
      provider: 'codex', codex_account: 'work', remember_provider: true,
    }));
    root.unmount();
  });

  it('confirmação manual de Claude não é trocada pela resposta tardia', async () => {
    const pending = deferred<Record<string, unknown>>();
    calls.providers.mockReturnValue(pending.promise);
    const { container, root } = await renderSheet(false, false);
    await openSettings(container);
    await act(async () => button(container, 'Claude')!.click());
    await act(async () => pending.resolve({ codex: { disponivel: true, default: true } }));
    await send(container);
    expect(calls.create).toHaveBeenCalledWith(server, expect.objectContaining({ provider: 'claude' }));
    root.unmount();
  });

  it('mostra erro da preferência e espera escolha explícita antes de enviar', async () => {
    calls.providers.mockRejectedValue(new Error('provider-probe-failed'));
    const { container, root } = await renderSheet(false, false);
    expect(container.textContent).toContain('provider-probe-failed');
    await send(container);
    expect(calls.create).not.toHaveBeenCalled();
    await openSettings(container);
    await act(async () => button(container, 'Codex')!.click());
    await send(container);
    expect(calls.create).toHaveBeenCalledWith(server, expect.objectContaining({ provider: 'codex' }));
    root.unmount();
  });

  it('abre os resumos e conserva destino, seleções e texto ao fechar a folha', async () => {
    calls.accounts.mockResolvedValue([defaultAccount, connected]);
    calls.models.mockResolvedValue({ models: [{ id: 'sol', name: 'Sol', efforts: ['low', 'high'] }], reduced: false });
    const container = document.createElement('div'); const root = createRoot(container);
    await act(async () => root.render(createElement(CreateSessionSheet)));
    await act(async () => type(container, 'primeira mensagem'));
    expect(button(container, 'native_new_chat_folder: repo')).toBeTruthy();
    calls.scan.mockResolvedValue({ entries: [{ name: 'Child', path: '/repo/child', is_git: true, has_claude_md: false }] });
    await openFolder(container);
    await act(async () => [...container.querySelectorAll('button')].find((button) => button.textContent?.includes('Child'))!.click());
    await flushDismiss();
    await openSettings(container);
    await act(async () => button(container, 'Codex')!.click());
    await closePanel(container, 'composer_session_settings');
    await pickAccount(container, 'Trabalho');
    await openSettings(container);
    await act(async () => button(container, 'Sol')!.click());
    await act(async () => button(container, 'reasoning_level_high')!.click());
    await closePanel(container, 'composer_session_settings');
    expect(button(container, 'native_new_chat_folder: child')).toBeTruthy();
    expect(button(container, 'composer_session_settings')!.textContent).toBe('Sol high');
    expect((container.querySelector('textarea[aria-label="nova_conversa_placeholder"]') as HTMLTextAreaElement).value).toBe('primeira mensagem');
    await act(async () => button(container, 'nova_conversa_enviar')!.click());
    for (let i = 0; i < 4; i++) await act(async () => Promise.resolve());
    expect(calls.create).toHaveBeenCalledWith(server, expect.objectContaining({
      cwd: '/repo/child', provider: 'codex', codex_account: 'work', model: 'sol', effort: 'high',
    }));
    expect(calls.create).toHaveBeenCalledTimes(1);
    expect(calls.send).toHaveBeenCalledWith(server, 'nova', 'primeira mensagem');
    expect(calls.send).toHaveBeenCalledTimes(1);
    expect(calls.replace).toHaveBeenCalledWith('/s/server-b/nova');
    root.unmount();
  });

  it('autoseleciona a primeira raiz utilizável sem abrir a folha e grava só depois do scan', async () => {
    const roots = deferred<{ name: string; path: string }[]>();
    const scan = deferred<{ entries: [] }>();
    calls.roots.mockReturnValue(roots.promise);
    calls.scan.mockImplementation(async (_server, _root, path) => path === '/missing' ? { entries: [], error: 'not_found' } : scan.promise);
    const container = document.createElement('div'); const root = createRoot(container);
    await act(async () => root.render(createElement(CreateSessionSheet)));
    await act(async () => type(container, 'primeira mensagem'));
    expect(container.querySelector('[data-folder-panel]')).toBeNull();
    expect(calls.pickerActive).toBe(1);
    // Carregando agora é esqueleto, sem texto: nada escolhível enquanto as raízes não chegam.
    expect(button(container, 'native_create_use_folder')).toBeUndefined();
    expect(button(container, 'Repo')).toBeUndefined();
    expect(button(container, 'nova_conversa_enviar')!.disabled).toBe(true);
    await act(async () => roots.resolve([{ name: 'Missing', path: '/missing' }, { name: 'Repo', path: '/repo' }]));
    expect(calls.scan).toHaveBeenCalledWith(server, '/missing', '/missing', expect.any(AbortSignal));
    expect(calls.scan).toHaveBeenCalledWith(server, '/repo', '/repo', expect.any(AbortSignal));
    expect(localStorage.getItem('create.project.v1:server-b')).toBeNull();
    await act(async () => scan.resolve({ entries: [] }));
    expect(button(container, 'native_new_chat_folder: repo')).toBeTruthy();
    expect((container.querySelector('textarea[aria-label="nova_conversa_placeholder"]') as HTMLTextAreaElement).value).toBe('primeira mensagem');
    expect(button(container, 'nova_conversa_enviar')!.disabled).toBe(false);
    expect(localStorage.getItem('create.project.v1:server-b')).toBe('{"root":"/repo","cwd":"/repo"}');
    expect(calls.create).not.toHaveBeenCalled();
    expect(calls.send).not.toHaveBeenCalled();
    expect(calls.pickerPeak).toBe(1);
    expect(calls.pickerActive).toBe(0);
    root.unmount();
  });

  it.each([
    ['gesto', false], ['controlado', false], ['gesto', true], ['controlado', true],
  ] as const)('fecha por %s com picked=%s sem dois pickers durante callback tardio', async (closing, picked) => {
    if (!picked) calls.roots.mockReturnValue(new Promise(() => {}));
    const container = document.createElement('div'); const root = createRoot(container);
    await act(async () => root.render(createElement(CreateSessionSheet)));
    await act(async () => type(container, 'texto conservado'));
    expect(calls.pickerActive).toBe(picked ? 0 : 1);
    const initialSignal = calls.roots.mock.calls[0][1] as AbortSignal;
    if (!picked) expect(initialSignal.aborted).toBe(false);
    await openFolder(container);
    const sheet = container.querySelector('[data-folder-panel]')!;
    expect(sheet).toBeTruthy();
    expect(sheet.querySelectorAll('[data-testid="cwd-picker"]').length).toBe(1);
    const sheetSignal = calls.roots.mock.calls.at(-1)![1] as AbortSignal;
    if (sheetSignal) {
      expect(initialSignal.aborted).toBe(true);
      expect(sheetSignal.aborted).toBe(false);
    }
    if (picked) expect(button(container, 'native_create_use_folder')).toBeTruthy();
    await act(async () => button(container, closing === 'gesto' ? 'back-panel' : 'close-panel')!.click());
    expect(calls.dismiss).toBeTruthy();
    expect(container.querySelector('[data-folder-panel]')).toBe(sheet);
    expect(sheet.getAttribute('data-open')).toBe('false');
    expect(calls.pickerActive).toBe(picked ? 0 : 1);
    expect(calls.pickerPeak).toBe(1);
    if (sheetSignal) expect(sheetSignal.aborted).toBe(true);
    expect(sheet.querySelector('[data-testid="cwd-picker"]')).toBeNull();
    expect(button(sheet as HTMLElement, 'native_create_use_folder')).toBeUndefined();
    if (picked) expect(button(container, 'native_new_chat_folder: repo')).toBeTruthy();
    else expect(container.querySelectorAll('[data-testid="cwd-picker"]').length).toBe(1);
    await flushDismiss();
    expect(container.querySelector('[data-folder-panel]')).toBeNull();
    expect(calls.pickerActive).toBe(picked ? 0 : 1);
    if (sheetSignal) expect(sheetSignal.aborted).toBe(true);
    expect(calls.pickerHistory.every((active) => active <= 1)).toBe(true);
    expect((container.querySelector('textarea[aria-label="nova_conversa_placeholder"]') as HTMLTextAreaElement).value).toBe('texto conservado');
    expect(calls.create).not.toHaveBeenCalled();
    expect(calls.send).not.toHaveBeenCalled();
    root.unmount();
    expect(calls.pickerActive).toBe(0);
    expect(calls.dismiss).toBeNull();
  });

  it.each(['raizes', 'scan', 'preferencia', 'catalogo', 'modelos'] as const)('mantém erro de %s e texto editável no principal fechado sem POST', async (failure) => {
    const message = failure === 'raizes' ? '401: unauthorized'
      : failure === 'scan' ? 'sem_permissao'
      : failure === 'preferencia' ? 'projeto_indisponivel'
      : failure === 'catalogo' ? 'catalogo_recusado' : 'modelos_recusados';
    if (failure === 'raizes') calls.roots.mockRejectedValue(new Error(message));
    if (failure === 'scan') calls.scan.mockResolvedValue({ entries: [], error: 'permission_denied' });
    if (failure === 'preferencia') {
      localStorage.setItem('create.project.v1:server-b', '{"root":"/repo","cwd":"/repo/deleted"}');
      calls.scan.mockResolvedValue({ entries: [], error: 'not_found' });
    }
    if (failure === 'catalogo' || failure === 'modelos') {
      calls.scan.mockReturnValue(new Promise(() => {}));
      if (failure === 'catalogo') calls.configs.mockRejectedValue(new Error(message));
      else calls.models.mockRejectedValue(new Error(message));
    }
    const container = document.createElement('div'); const root = createRoot(container);
    await act(async () => root.render(createElement(CreateSessionSheet)));
    await act(async () => type(container, 'antes da folha'));
    expect(container.textContent).toContain(message);
    expect(container.querySelector('[data-folder-panel]')).toBeNull();
    await openOptions(container);
    await closePanel(container, 'composer_session_settings');
    await act(async () => type(container, 'depois da folha'));
    expect(container.textContent).toContain(message);
    expect((container.querySelector('textarea[aria-label="nova_conversa_placeholder"]') as HTMLTextAreaElement).value).toBe('depois da folha');
    expect(button(container, 'nova_conversa_enviar')!.disabled).toBe(true);
    await act(async () => button(container, 'nova_conversa_enviar')!.click());
    expect(calls.create).not.toHaveBeenCalled();
    expect(calls.send).not.toHaveBeenCalled();
    root.unmount();
  });

  it('mantém erro da conta no principal após gesto tardio e bloqueia envio sem apagar texto', async () => {
    calls.accounts.mockRejectedValue(new Error('conta_recusada'));
    const { container, root } = await renderSheet();
    await act(async () => type(container, 'texto da conta'));
    await openOptions(container);
    await closePanel(container, 'composer_session_settings');
    expect(container.textContent).toContain('conta_recusada');
    expect(button(container, 'native_new_chat_folder: repo')).toBeTruthy();
    await act(async () => type(container, 'texto corrigido'));
    expect((container.querySelector('textarea[aria-label="nova_conversa_placeholder"]') as HTMLTextAreaElement).value).toBe('texto corrigido');
    expect(button(container, 'nova_conversa_enviar')!.disabled).toBe(true);
    expect(calls.create).not.toHaveBeenCalled();
    expect(calls.send).not.toHaveBeenCalled();
    root.unmount();
  });

  it('confere preferência Windows sem reconstruir o caminho', async () => {
    const path = 'C:\\work\\repo';
    localStorage.setItem('create.project.v1:server-b', JSON.stringify({ root: 'C:\\work', cwd: path }));
    calls.roots.mockResolvedValue([{ name: 'Work', path: 'C:\\work' }]);
    const { container, root } = await renderSheet();
    await openOptions(container);
    expect(container.textContent).toContain(path);
    expect(calls.scan).toHaveBeenCalledWith(server, 'C:\\work', path, expect.any(AbortSignal));
    root.unmount();
  });

  it('valida a subpasta escolhida e restaura ao reabrir', async () => {
    const { container, root } = await renderSheet();
    calls.scan.mockResolvedValue({ entries: [{ name: 'Child', path: '/repo/child', is_git: true, has_claude_md: false }] });
    await openFolder(container);
    await act(async () => [...container.querySelectorAll('button')].find((button) => button.textContent?.includes('Child'))!.click());
    await flushDismiss();
    await openOptions(container);
    expect(container.textContent).toContain('/repo/child');
    expect(localStorage.getItem('create.project.v1:server-b')).toBe('{"root":"/repo","cwd":"/repo/child"}');
    root.unmount();
    const reopened = await renderSheet();
    await openOptions(reopened.container);
    expect(reopened.container.textContent).toContain('/repo/child');
    reopened.root.unmount();
  });

  it('cria Claude no destino explícito do projeto', async () => {
    const { container, root } = await renderSheet();
    await openSettings(container);
    await act(async () => button(container, 'Claude')!.click());
    await openOptions(container);
    await act(async () => {
      const field = container.querySelector('textarea[aria-label="comum_nome"]') as HTMLTextAreaElement;
      field.value = 'meu-nome';
      field.dispatchEvent(new Event('input', { bubbles: true }));
    });
    await send(container);
    expect(calls.create).toHaveBeenCalledWith(server, expect.objectContaining({ provider: 'claude', cwd: '/repo', name: 'meu-nome' }));
    expect(calls.send).toHaveBeenCalledWith(server, 'nova', 'oi');
    expect(calls.replace).toHaveBeenCalledWith('/s/server-b/nova');
    root.unmount();
  });

  it('diz por que a cota da conta não atualizou ao lado dos números antigos', async () => {
    calls.configs.mockResolvedValue([{ path: '/a', label: 'Conta A', active: true }, { path: '/b', label: 'Conta B', active: false }]);
    const antiga: CotaContaResumo = { id: 'claude:/a', estado: 'lida', janelas: [{ rotulo: '5h', pct: 42 }], motivo: 'sem-resposta' };
    calls.cotas = [antiga];
    const { container, root } = await renderSheet(false, false);
    await openOptions(container);
    expect(motivoCotaAntiga(antiga)).toBeTruthy();
    expect(container.textContent).toContain(`5h 42% · ${motivoCotaAntiga(antiga)}`);
    root.unmount();
  });

  it('mostra preferência indisponível sem substituir a pasta', async () => {
    localStorage.setItem('create.project.v1:server-b', '{"root":"/repo","cwd":"/repo/deleted"}');
    calls.scan.mockResolvedValue({ entries: [], error: 'not_found' });
    const container = document.createElement('div'); const root = createRoot(container);
    await act(async () => root.render(createElement(CreateSessionSheet)));
    expect(container.textContent).toContain('projeto_indisponivel');
    expect(container.textContent).toContain('native_create_computer_folder');
    expect(calls.create).not.toHaveBeenCalled();
    expect(calls.save).not.toHaveBeenCalled();
    root.unmount();
  });

  it('preserva erro de autenticação legível no seletor', async () => {
    calls.roots.mockRejectedValue(new Error('401: unauthorized'));
    const container = document.createElement('div'); const root = createRoot(container);
    await act(async () => root.render(createElement(CreateSessionSheet)));
    expect(container.textContent).toContain('401: unauthorized');
    expect(container.textContent).toContain('native_create_computer_folder');
    root.unmount();
  });

  it('descarta scan, contas Claude e modelos tardios ao mudar de máquina', async () => {
    const scan = deferred<{ entries: [] }>(); const models = deferred<{ models: { id: string; name: string }[]; reduced: boolean }>();
    const configs = deferred<{ path: string; label: string; active: boolean }[]>();
    calls.configs.mockReturnValueOnce(configs.promise);
    calls.scan.mockReturnValueOnce(scan.promise); calls.models.mockReturnValueOnce(models.promise);
    const container = document.createElement('div'); const root = createRoot(container);
    await act(async () => root.render(createElement(CreateSessionSheet)));
    calls.target = { id: 'server-c', label: 'Servidor C', baseUrl: 'https://c.local', token: 'token-c' };
    calls.roots.mockResolvedValue([{ name: 'Other', path: '/other' }]);
    await act(async () => root.render(createElement(CreateSessionSheet)));
    await act(async () => button(container, 'native_new_chat_machine: Servidor B')!.click());
    await act(async () => button(container, 'Servidor C')!.click());
    await openOptions(container);
    await act(async () => { scan.resolve({ entries: [] }); configs.resolve([{ path: '/old', label: 'Conta antiga', active: true }]); models.resolve({ models: [{ id: 'old', name: 'Modelo antigo' }], reduced: false }); });
    expect(container.textContent).toContain('/other');
    expect(container.textContent).not.toContain('/repo');
    expect(container.textContent).not.toContain('Modelo antigo');
    expect(calls.models).not.toHaveBeenCalledWith(calls.target, 'claude', null, '/old', null, expect.any(AbortSignal));
    expect(localStorage.getItem('create.project.v1:server-b')).toBeNull();
    root.unmount();
  });

  it('informa falha ao salvar preferência sem impedir seleção válida', async () => {
    calls.save.mockImplementation(() => { throw new Error('storage failed'); });
    const { container, root } = await renderSheet();
    expect(container.textContent).toContain('projeto_salvar_erro');
    await openOptions(container);
    expect(container.textContent).toContain('/repo');
    root.unmount();
  });

  it('seleciona conta do servidor e envia a conta no create', async () => {
    const { container, root } = await renderSheet();
    await act(async () => Promise.resolve());
    await send(container);
    expect(calls.create).toHaveBeenCalledWith(server, expect.objectContaining({ provider: 'codex', codex_account: 'work' }));
    root.unmount();
  });

  it('abre a sessão sem esperar a sincronização da conta na tela', async () => {
    calls.prepare.mockReturnValue(new Promise(() => {}));
    const { container, root } = await renderSheet();
    await send(container);
    expect(calls.replace).toHaveBeenCalledWith('/s/server-b/nova');
    root.unmount();
  });

  it('Conferir mostra só as três últimas mensagens do usuário sem repetir criação ou envio', async () => {
    calls.send.mockRejectedValueOnce(new TypeError('offline'));
    calls.history.mockResolvedValue([
      { kind: 'user_msg', id: 'u1', text: 'mensagem mais antiga' },
      { kind: 'user_msg', id: 'u2', text: 'primeira recebida' },
      { kind: 'assistant_msg', id: 'a1', text: 'resposta do assistente' },
      { kind: 'user_msg', id: 'u3', text: 'segunda recebida' },
      { kind: 'user_msg', id: 'u4', text: 'terceira recebida' },
    ]);
    const { container, root } = await renderSheet();
    await send(container);
    expect(container.textContent).not.toContain('new_conversation_received_messages');
    await act(async () => button(container, 'nova_conversa_conferir')!.click());
    expect(container.textContent).toContain('new_conversation_received_messages');
    expect(container.textContent).toContain('primeira recebida');
    expect(container.textContent).toContain('segunda recebida');
    expect(container.textContent).toContain('terceira recebida');
    expect(container.textContent).not.toContain('mensagem mais antiga');
    expect(container.textContent).not.toContain('resposta do assistente');
    expect(button(container, 'nova_conversa_enviar')!.disabled).toBe(true);
    expect(calls.create).toHaveBeenCalledOnce();
    expect(calls.send).toHaveBeenCalledOnce();
    root.unmount();
  });

  // Cada linha da tabela é a lista de argumentos: o histórico vai embrulhado num array a mais.
  it.each([[[]], [[{ kind: 'assistant_msg', id: 'a1', text: 'só resposta' }]]])(
    'Conferir sem user_msg mostra vazio; falha posterior continua visível (%j)', async (events) => {
      calls.send.mockRejectedValueOnce(new TypeError('offline'));
      calls.history.mockResolvedValueOnce(events).mockRejectedValueOnce(new Error('consulta indisponível'));
      const { container, root } = await renderSheet();
      await send(container);
      await act(async () => button(container, 'nova_conversa_conferir')!.click());
      expect(container.textContent).toContain('new_conversation_no_received_messages');
      expect(container.textContent).not.toContain('só resposta');
      await act(async () => button(container, 'nova_conversa_conferir')!.click());
      expect(container.textContent).toContain('nova_conversa_envio_conferir_erro:consulta indisponível');
      expect(container.textContent).not.toContain('new_conversation_no_received_messages');
      expect(calls.create).toHaveBeenCalledOnce();
      expect(calls.send).toHaveBeenCalledOnce();
      root.unmount();
    },
  );

  it('cria normalmente sob StrictMode após o ciclo de efeitos', async () => {
    const { container, root } = await renderSheet(true);
    await send(container);
    expect(calls.create).toHaveBeenCalledWith(server, expect.objectContaining({ provider: 'codex', codex_account: 'work' }));
    expect(calls.replace).toHaveBeenCalled();
    root.unmount();
  });

  it('envia esforço Codex do modelo escolhido e limpa esforço incompatível', async () => {
    calls.models.mockResolvedValue({ models: [
      { id: 'sol', name: 'Sol', efforts: ['low', 'high'] },
      { id: 'mini', name: 'Mini', efforts: ['low'] },
    ], reduced: false });
    const { container, root } = await renderSheet();
    await openSettings(container);
    await act(async () => button(container, 'Sol')!.click());
    await act(async () => button(container, 'reasoning_level_high')!.click());
    await send(container);
    expect(calls.create).toHaveBeenCalledWith(server, expect.objectContaining({ model: 'sol', effort: 'high', codex_account: 'work' }));
    calls.create.mockClear();
    await act(async () => button(container, 'nova_conversa_descartar')!.click());
    await openSettings(container);
    await act(async () => button(container, 'Mini')!.click());
    await send(container);
    expect(calls.create).toHaveBeenCalledWith(server, expect.objectContaining({ model: 'mini', effort: null, codex_account: 'work' }));
    root.unmount();
  });

  it('retoma conversa Codex com a identidade do ArchiveEntry', async () => {
    calls.archives.mockResolvedValue([archived]);
    const { container, root } = await renderSheet();
    await act(async () => Promise.resolve());
    await openOptions(container);
    await act(async () => button(container, 'criar_retomar_escolha')!.click());
    expect(container.textContent).toContain('última mensagem');
    const option = [...container.querySelectorAll('button')].find((button) => button.textContent === 'última mensagem');
    expect(option).toBeTruthy();
    await act(async () => option!.click());
    const resume = [...container.querySelectorAll('button')].find((button) => button.textContent === 'criar_retomar_acao');
    expect(resume).toBeTruthy();
    await act(async () => resume!.click());
    expect(calls.resume).toHaveBeenCalledWith('-repo', 'sid-1', null, null, 'codex', 'work', server);
    expect(calls.prepare).not.toHaveBeenCalled();
    expect(calls.create).not.toHaveBeenCalled();
    root.unmount();
  });

  it('não navega se desmontar depois que o create fica pendente', async () => {
    const pending = deferred<{ name: string; state: 'idle' }>();
    calls.create.mockReturnValue(pending.promise);
    const { container, root } = await renderSheet();
    await send(container);
    root.unmount();
    await act(async () => pending.resolve({ name: 'nova', state: 'idle' }));
    for (let i = 0; i < 4; i++) await act(async () => Promise.resolve());
    expect(calls.replace).not.toHaveBeenCalled();
    expect(calls.send).toHaveBeenCalledWith(server, 'nova', 'oi');
  });

  it('libera a retomada ao trocar de conta e descarta a operação antiga', async () => {
    calls.accounts.mockResolvedValue([connected, defaultAccount]);
    calls.archives.mockResolvedValue([archived]);
    const pending = deferred<{ name: string; state: 'idle' }>();
    calls.resume.mockReturnValue(pending.promise);
    const { container, root } = await renderSheet();
    await pickAccount(container, 'Trabalho');
    await act(async () => Promise.resolve());
    await act(async () => button(container, 'criar_retomar')!.click());
    await act(async () => button(container, 'última mensagem')!.click());
    expect(calls.resume).toHaveBeenCalledOnce();
    await pickAccount(container, 'Padrão');
    await act(async () => type(container, 'oi'));
    expect(button(container, 'nova_conversa_enviar')!.disabled).toBe(false);
    await act(async () => pending.resolve({ name: 'retomada', state: 'idle' }));
    expect(calls.replace).not.toHaveBeenCalled();
    root.unmount();
  });

  it('não navega se desmontar enquanto a retomada está pendente', async () => {
    calls.archives.mockResolvedValue([archived]);
    const pending = deferred<{ name: string; state: 'idle' }>();
    calls.resume.mockReturnValue(pending.promise);
    const { container, root } = await renderSheet();
    expect(button(container, 'última mensagem')).toBeUndefined();
    await act(async () => button(container, 'criar_retomar')!.click());
    await act(async () => button(container, 'última mensagem')!.click());
    expect(calls.resume).toHaveBeenCalledOnce();
    root.unmount();
    await act(async () => pending.resolve({ name: 'retomada', state: 'idle' }));
    expect(calls.replace).not.toHaveBeenCalled();
  });
  it('mostra o campo e aceita texto enquanto as raízes carregam, sem enviar sem destino', async () => {
    calls.roots.mockReturnValue(new Promise(() => {}));
    const container = document.createElement('div'); const root = createRoot(container);
    await act(async () => root.render(createElement(CreateSessionSheet)));
    await act(async () => type(container, 'primeira'));
    expect((container.querySelector('textarea[aria-label="nova_conversa_placeholder"]') as HTMLTextAreaElement).value).toBe('primeira');
    expect(button(container, 'nova_conversa_enviar')!.disabled).toBe(true);
    expect(container.textContent).toContain('nova_conversa_sem_destino');
    expect(container.textContent).toContain('Servidor B');
    root.unmount();
  });

  it('reabre tentativa gravada em criação como incerta, sem novo POST', async () => {
    localStorage.setItem('create.attempt.v1:server-b', JSON.stringify({
      id: 'a1', serverId: 'server-b', text: 'guardada', phase: 'creating', sessionName: null,
      body: { name: 'repo-a1', cwd: '/repo', provider: 'claude' },
    }));
    const { container, root } = await renderSheet();
    expect(container.textContent).toContain('nova_conversa_guardada');
    expect(container.textContent).toContain('nova_conversa_criacao_incerta');
    expect(button(container, 'nova_conversa_conferir')).toBeTruthy();
    expect(button(container, 'nova_conversa_enviar')!.disabled).toBe(true);
    expect(calls.create).not.toHaveBeenCalled();
    root.unmount();
  });

  it('mensagem já entregue não volta ao campo ao reabrir, nem depois de descartar', async () => {
    localStorage.setItem('create.attempt.v1:server-b', JSON.stringify({
      id: 'a2', serverId: 'server-b', text: 'entregue', phase: 'sent', sessionName: 'nova',
      body: { name: 'repo-a2', cwd: '/repo', provider: 'claude' },
    }));
    const { container, root } = await renderSheet();
    const input = container.querySelector('textarea[aria-label="nova_conversa_placeholder"]') as HTMLTextAreaElement;
    expect(input.value).toBe('');
    expect(container.textContent).toContain('nova_conversa_guardada');
    expect(button(container, 'nova_conversa_abrir')).toBeTruthy();
    await act(async () => button(container, 'nova_conversa_descartar')!.click());
    expect(input.value).toBe('');
    expect(button(container, 'nova_conversa_enviar')!.disabled).toBe(true);
    expect(calls.create).not.toHaveBeenCalled();
    root.unmount();
  });

  it('toque duplo em Enviar dispara um único create', async () => {
    const pending = deferred<{ name: string; state: 'idle' }>();
    calls.create.mockReturnValue(pending.promise);
    const { container, root } = await renderSheet();
    await act(async () => type(container, 'oi'));
    const enviar = button(container, 'nova_conversa_enviar')!;
    await act(async () => { enviar.click(); enviar.click(); });
    await act(async () => pending.resolve({ name: 'nova', state: 'idle' }));
    for (let i = 0; i < 4; i++) await act(async () => Promise.resolve());
    expect(calls.create).toHaveBeenCalledTimes(1);
    expect(calls.send).toHaveBeenCalledTimes(1);
    root.unmount();
  });

  it('cwd recusado mantém o texto editável com o motivo e não navega', async () => {
    calls.create.mockRejectedValueOnce(Object.assign(new Error('400: diretório não existe'), { status: 400 }));
    const { container, root } = await renderSheet();
    await send(container);
    const input = container.querySelector('textarea[aria-label="nova_conversa_placeholder"]') as HTMLTextAreaElement;
    expect(input.value).toBe('oi');
    expect(container.textContent).toContain('400: diretório não existe');
    expect(button(container, 'nova_conversa_enviar')!.disabled).toBe(false);
    expect(calls.replace).not.toHaveBeenCalled();
    expect(calls.send).not.toHaveBeenCalled();
    root.unmount();
  });

  it('create OK e input recusado abre a mesma sessão e não cria de novo', async () => {
    calls.send.mockRejectedValueOnce(Object.assign(new Error('recusado'), { status: 400 }));
    const { container, root } = await renderSheet();
    await send(container);
    expect(calls.create).toHaveBeenCalledTimes(1);
    expect(calls.replace).toHaveBeenCalledWith('/s/server-b/nova');
    await act(async () => button(container, 'nova_conversa_reenviar')!.click());
    for (let i = 0; i < 4; i++) await act(async () => Promise.resolve());
    expect(calls.create).toHaveBeenCalledTimes(1);
    expect(calls.send).toHaveBeenCalledTimes(2);
    expect(calls.send).toHaveBeenLastCalledWith(server, 'nova', 'oi');
    root.unmount();
  });
});
