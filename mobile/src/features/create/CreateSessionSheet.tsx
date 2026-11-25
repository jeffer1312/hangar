import { Children, Fragment, useEffect, useState, useCallback, useRef, type ComponentProps, type ReactNode } from 'react';
import { Icon } from '../../ui/Icon';
import { ProviderGlyph } from '../../ui/ProviderGlyph';
import { AccessibilityInfo, ActivityIndicator, AppState, BackHandler, Platform, Pressable, ScrollView, Text, TextInput, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { useRouter } from 'expo-router';
import { resetCreatedChat } from '../../stores/chat';
import { getArchivePorCwd, getCodexAccountsForServer, defaultCodexAccount, getAntigravityAccountsForServer,
  getEnginesForServer, getSessionDefaultsForServer, getConfigForServer, getMeForServer, fetchSessionsForServer, listClaudeConfigsForServer, probeServerResponse,
  modelOptionsForServer, resumeArchivedConversation, getProvidersForServer, createSessionForServer, sanitizeSessionName, getWorktreesForServer,
  commitsLeftOut, folderGitActionForServer, freshBase, getFolderBranchesForServer,
  PrewarmReserve, prewarmSessionForServer, releasePrewarmForServer, getHistory, getRootsForServer, pickFolderRoot } from '@hangar/core';
import { basename, ctxWindow, providerName, contaComFolga, cotaDaConta, cotaParada, janelaEsgotada, motivoCotaAntiga, resumoCota, CLAUDE_PERMISSION_MODES, EFFORT_LEVELS,
  CREATE_SESSION_PROVIDERS, canChooseSurface, permissionModes, effortLevels, contextRefsPlainText, machineLabel, serverColor, tagHex } from '@hangar/core';
import type { AccountBadge } from '@hangar/core';
import { useShallow } from 'zustand/react/shallow';
import { useSessions } from '../../stores/sessions';
import { MACHINE_ICON_NAME } from '../tags/tagVisuals';
import { AccountGlyph } from '../sessions/SessionPlate';
import type { AntigravityAccount, ArchiveEntry, CodexAccount, ConfigDirInfo, Provider, ModelOption, CotaContaResumo, Server, SessionDefaults, WorktreeChoice, WorktreeRepo } from '@hangar/core';
import { SessionSettingsControl } from '../../chat/SessionSettings';
import { semEsforco } from '../pills/pills';
import { SettingsList } from '../../ui/SettingsList';
import { useServers } from '../../stores/servers';
import { readProject, rememberProject } from '../../stores/createPreferences';
import { autoSessionName, NO_PROJECT_CWD, type NewConversationInput } from '../../stores/newConversation';
import { CwdPicker } from './CwdPicker';
import { MachineMenu, MachineTile } from './MachineMenu';
import { isDemoUrl } from '../../net/demo';
import { recentCwds, recentProjects, samePath } from './folderGroups';
import { loadErrorText } from './loadError';
import { useCanceledLoadRetry } from './useCanceledLoadRetry';
import { ProviderLogoTabs, ProviderTrack } from './ProviderChoice';
import { DefaultChecks, useDefaultChecks } from './DefaultChecks';
import { BranchPicker } from './BranchPicker';
import { CodexContextControl, useCodexContextControl } from './CodexContextControl';
import { NewConversation, type HomePresentation } from './NewConversation';
import { SPEED_TIERS, speedHint, speedLabel, type Tier } from '../pills/serviceTierControl';
import { QuietPill } from './QuietPill';
import { SettingRow } from './SettingRow';
import { readMachine, rememberMachine } from './choicesPrefs';
import { superficie } from '../../theme/superficie';
import { AnchoredPanel } from '../../ui/AnchoredPanel';
import * as m from '../../paraglide/messages';

type ProviderProbe = Record<string, { disponivel: boolean; motivo: string | null; default?: boolean }>;
type Choice = Omit<ComponentProps<typeof QuietPill>, 'ref'>;
// Em cima só o vão até a pílula da máquina: mais que isso roubaria o toque dela.
const LINK_SLOP = { top: 4, bottom: 12, left: 12, right: 12 };

function valorModelo(mm: ModelOption): string {
  return mm.provider ? `${mm.provider}/${mm.id}` : mm.id;
}

function PanelSelect({
  value,
  options,
  onChange,
  placeholder,
}: {
  value: string;
  options: { value: string; label: string; hint?: string }[];
  onChange: (v: string) => void;
  placeholder?: string;
}) {
  const [open, setOpen] = useState(false);
  const anchor = useRef<View>(null);
  const label = options.find((o) => o.value === value)?.label ?? placeholder ?? '—';
  return <>
    <Pressable ref={anchor} onPress={() => setOpen(true)} style={styles.selectBtn} accessibilityRole="button" accessibilityLabel={label}>
      <Text style={styles.selectTxt} numberOfLines={1}>{label}</Text>
      <View style={styles.selectChevron}><Icon name="ChevronsUpDown" size={16} /></View>
    </Pressable>
    <AnchoredPanel open={open} anchor={anchor.current} onClose={() => setOpen(false)} label={label} closeLabel={m.native_close()} radius={12} padding={4}>
      <SettingsList items={options.map((o) => ({ id: o.value, label: o.label, hint: o.hint, selected: o.value === value }))}
        onPick={(id) => { onChange(id); setOpen(false); }} />
    </AnchoredPanel>
  </>;
}

// `layout: 'form'`: o formulário completo em linhas (Nova sessão do menu); o padrão é a conversa nova da inicial.
export function CreateSessionSheet({ onClose, keyboardOffset, home, layout = 'conversation', initial }: {
  onClose?: () => void; keyboardOffset?: number; home?: HomePresentation; layout?: 'conversation' | 'form';
  /** Quem abre já sabendo onde e com qual motor do Claude (o assistente do CLIProxyAPI); não vira escolha lembrada. */
  initial?: { server?: string; engine?: string };
}) {
  const servers = useServers((s) => s.servers);
  const activeId = useServers((s) => s.activeId);
  // A máquina escolhida na pílula vale só para onde a conversa nasce; o servidor ativo é das Configurações.
  const [machine, setMachine] = useState(() => {
    const remembered = readMachine();
    return servers.find((s) => s.id === initial?.server)?.id ?? servers.find((s) => s.id === remembered)?.id
      ?? servers.find((s) => s.id === activeId)?.id ?? servers[0]?.id ?? null;
  });
  const target = servers.find((s) => s.id === machine) ?? servers.find((s) => s.id === activeId) ?? servers[0];
  // Fixa também o destino que apareceu após a carga das máquinas ou a remoção da escolhida.
  useEffect(() => { if (target && target.id !== machine) setMachine(target.id); }, [machine, target?.id]);
  const pickMachine = useCallback((id: string) => {
    rememberMachine(id);
    setMachine(id);
  }, []);
  // O motor é de uma máquina: trocar a pílula de máquina descarta o pedido.
  const initialEngine = initial?.engine && (!initial.server || initial.server === target?.id) ? initial.engine : undefined;
  return target ? <CreateSessionForm key={`${target.id}:${target.baseUrl}`} active={target} machines={servers} onPickMachine={pickMachine} onClose={onClose} keyboardOffset={keyboardOffset} home={home} layout={layout} initialEngine={initialEngine} />
    : home?.mode === 'sessions' ? <View style={{ flex: 1 }}>{home.sessions}</View>
      : <Text accessibilityRole="alert">{m.servidor_nao_existe()}</Text>;
}

function CreateSessionForm({ active, machines, onPickMachine, onClose, keyboardOffset, home, layout, initialEngine }: {
  active: Server; machines: Server[]; onPickMachine: (id: string) => void; onClose?: () => void; keyboardOffset?: number; home?: HomePresentation;
  layout: 'conversation' | 'form'; initialEngine?: string;
}) {
  const router = useRouter();
  const { theme } = useUnistyles();
  // Etiquetas das máquinas (a mesma fonte da lista de sessões); valor estável, sem redesenhar a cada evento.
  const machineTags = useSessions(useShallow((s) => machines.map((sv) => s.tags?.[sv.id] ?? null)));
  const machineOffline = useSessions(useShallow((s) => machines.map((sv) => s.byServer.find((b) => b.server.id === sv.id)?.error === 'offline')));
  const [initialized, setInitialized] = useState(home?.mode !== 'sessions');
  const enabled = initialized || home?.mode !== 'sessions';
  useEffect(() => {
    if (home?.mode !== 'sessions') setInitialized(true);
  }, [home?.mode]);
  // Modelo e nível escolhidos nesta abertura ou o padrão do servidor; reaplicado quando a lista do provider recarrega.
  const desired = useRef({ model: '', effort: '' });
  // Aplicado uma vez, quando a lista de motores chega; depois a escolha é da pessoa.
  const seededEngine = useRef(initialEngine ?? '');
  // Aberto pelo assistente com um motor: criar (e reservar) antes de ele chegar abriria na conta Claude direta.
  const [seedPending, setSeedPending] = useState(!!initialEngine);

  const [picked, setPicked] = useState<string | null>(null);
  // Worktree escolhida no menu de branch: o repositório dela vem junto, antes de a lista geral carregar.
  const [folderRepo, setFolderRepo] = useState<{ path: string; repo: string } | null>(null);
  const [name, setName] = useState('');
  const [worktree, setWorktree] = useState<WorktreeChoice | null>(null);
  const onWorktree = useCallback((v: WorktreeChoice | null) => setWorktree(v), []);
  // Fora do painel, que desmonta ao fechar: `worktree.base` sozinho não diz se a base foi escolhida ou é o padrão.
  const basePicked = useRef<string | null>(null);
  const [branchTaken, setBranchTaken] = useState(false);
  const [branchOpen, setBranchOpen] = useState(false);
  const [folderOpen, setFolderOpen] = useState(false);
  const [creating, setCreating] = useState(false);
  const [repos, setRepos] = useState<WorktreeRepo[]>([]);
  const [hasSameFolder, setHasSameFolder] = useState(false);
  const [error, setError] = useState('');
  const [configsError, setConfigsError] = useState('');
  const [enginesError, setEnginesError] = useState('');
  const catalogError = [configsError, enginesError].filter(Boolean).join('\n');
  const [configsTry, setConfigsTry] = useState(0);
  const [enginesTry, setEnginesTry] = useState(0);
  const [providersTry, setProvidersTry] = useState(0);
  const [modelsTry, setModelsTry] = useState(0);
  const [codexTry, setCodexTry] = useState(0);
  const startConfigsRetry = useCanceledLoadRetry(useCallback(() => setConfigsTry((n) => n + 1), []));
  const startEnginesRetry = useCanceledLoadRetry(useCallback(() => setEnginesTry((n) => n + 1), []));
  const startProvidersRetry = useCanceledLoadRetry(useCallback(() => setProvidersTry((n) => n + 1), []));
  const startModelsRetry = useCanceledLoadRetry(useCallback(() => setModelsTry((n) => n + 1), []));
  const startCodexRetry = useCanceledLoadRetry(useCallback(() => setCodexTry((n) => n + 1), []));
  const [agyTry, setAgyTry] = useState(0);
  const startAgyRetry = useCanceledLoadRetry(useCallback(() => setAgyTry((n) => n + 1), []));
  const [projectWarning, setProjectWarning] = useState('');
  const pickGeneration = useRef(0);

  // O inicial vem da sonda de providers, que segue o último provedor que o servidor guardou.
  const [provider, setProvider] = useState<Provider>('claude');
  // Sem terminal é o `headless_default` do servidor; só vai no pedido o que a pessoa escolher aqui.
  const [headlessPick, setHeadlessPick] = useState<boolean | null>(null);
  // Convidado não lê a configuração do dono: como no desktop, o servidor decide e a tela mostra sem terminal.
  const [modeRead, setModeRead] = useState<{ state: 'loading' | 'guest' } | { state: 'value'; value: boolean } | { state: 'error'; error: string }>({ state: 'loading' });
  const [modeTry, setModeTry] = useState(0);
  const startModeRetry = useCanceledLoadRetry(useCallback(() => setModeTry((n) => n + 1), []));
  const serverHeadless = modeRead.state === 'value' ? modeRead.value : null;
  // Sem escolha nem leitura, não há modo a mostrar nem pedido a montar.
  const modeUnknown = headlessPick === null && (modeRead.state === 'loading' || modeRead.state === 'error');
  const headless = headlessPick ?? serverHeadless ?? true;
  const [providerProbe, setProviderProbe] = useState<ProviderProbe | null>(null);
  const [providersError, setProvidersError] = useState('');
  const providerTouched = useRef(false);
  const [providersLoading, setProvidersLoading] = useState(true);
  const [modelsLoading, setModelsLoading] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [extrasOpen, setExtrasOpen] = useState(false);
  const contextOpen = provider === 'codex' && extrasOpen && (settingsOpen || layout === 'form');
  const context = useCodexContextControl(active, contextOpen);
  const contextBusy = context.busy;
  const [catalogFast, setCatalogFast] = useState(false);
  const [catalogDefault, setCatalogDefault] = useState('');
  const [serviceTier, setServiceTier] = useState<'priority' | 'ultrafast' | undefined>();

  const [codexAccounts, setCodexAccounts] = useState<CodexAccount[]>([]);
  const [codexAccount, setCodexAccount] = useState('');
  const [codexLoading, setCodexLoading] = useState(false);
  const [codexError, setCodexError] = useState('');
  const [codexProgress, setCodexProgress] = useState('');
  const codexGeneration = useRef(0);
  const codexController = useRef<AbortController | null>(null);
  // Conta Google do Antigravity: só a padrão, porque o agy com terminal ignora a pasta de outra conta.
  // `agyAccount` fica vazio até a lista responder ou falhar, e segura a criação até lá.
  const [agyAccounts, setAgyAccounts] = useState<AntigravityAccount[]>([]);
  const [agyAccount, setAgyAccount] = useState('');
  const [agyInstalled, setAgyInstalled] = useState(true);
  const [agyLoading, setAgyLoading] = useState(false);
  const [agyError, setAgyError] = useState('');
  const mounted = useRef(true);
  const [retomaveis, setRetomaveis] = useState<ArchiveEntry[]>([]);
  const [retomavel, setRetomavel] = useState('');
  const [retomando, setRetomando] = useState(false);
  const archiveGeneration = useRef(0);
  const archiveController = useRef<AbortController | null>(null);

  const [configs, setConfigs] = useState<ConfigDirInfo[]>([]);
  const [configsLoading, setConfigsLoading] = useState(true);
  // Cota por conta no seletor (/api/cotas, chave `claude:<path>`); falha = lista sem número.
  const [cotas, setCotas] = useState<CotaContaResumo[]>([]);
  const [selectedConfig, setSelectedConfig] = useState<string | null>(null);
  const cotaSelecionada = selectedConfig ? cotaDaConta(cotas, selectedConfig) : undefined;
  // Só a escolha à mão no menu de conta desliga a troca automática por cota esgotada.
  const accountTouched = useRef(false);
  // D-003: só vai no pedido a conta que a pessoa escolheu nesta tela. A ativa sem escolha não vai, e o
  // servidor troca a que estiver a 95% ou mais. A que a tela trocou por cota esgotada vai como a que a
  // tela mostra: a regra é a mesma do servidor.
  const [accountChosen, setAccountChosen] = useState(false);
  // Conta (caminho) que a troca por cota deixou para trás; a linha acima da pílula oferece abrir nela.
  const [switchedFrom, setSwitchedFrom] = useState('');
  // A conta a 95% ou mais em que a pessoa mandou abrir: vai `force_account` enquanto ela for a escolhida.
  const [forcedPath, setForcedPath] = useState('');
  const forceAccount = !!forcedPath && forcedPath === selectedConfig;
  const contaLabel = (path: string) => configs.find((c) => c.path === path)?.label ?? basename(path);
  const [motores, setMotores] = useState<Awaited<ReturnType<typeof getEnginesForServer>>['motores']>({});
  const [engine, setEngine] = useState('');
  const [modelos, setModelos] = useState<ModelOption[]>([]);
  const [modelo, setModelo] = useState('');
  const [subagente, setSubagente] = useState('');
  const [esforco, setEsforco] = useState('');
  const [permissao, setPermissao] = useState('');
  const [loadedDefaultsKey, setLoadedDefaultsKey] = useState<string | null>(null);
  // O padrão gravado no servidor para o escopo da chave: marca a caixa "Usar como padrão".
  const [savedDefault, setSavedDefault] = useState<{ key: string; value: SessionDefaults | null } | null>(null);
  const [defaultsError, setDefaultsError] = useState('');
  const [defaultsTry, setDefaultsTry] = useState(0);
  const engineConfig = provider === 'claude' ? motores[engine] : undefined;
  const proxyEngine = !!(engineConfig?.cliproxy_accounts || engineConfig?.cliproxy_binding);
  const boundAccount = engineConfig?.cliproxy_bound_account;
  const engineAccount = boundAccount?.account;
  const bindingBlocked = !!engineConfig?.cliproxy_binding && !engineAccount;
  const proxyQuota = engineConfig?.cliproxy_binding ? cotas.find(c => c.id === `cliproxy-binding:${engineConfig.cliproxy_binding}`) : undefined;
  const defaultsKey = `${provider}:${provider === 'claude' ? engine : ''}:${engineAccount ?? ''}:${engineConfig?.cliproxy_binding ?? ''}`;
  const defaultsReady = loadedDefaultsKey === defaultsKey;
  const previousDefaults = useRef<{ key: string; engine: string | null } | null>(null);
  const choiceTouched = useRef(false);
  const permissionTouched = useRef(false);
  const [listaReduzida, setListaReduzida] = useState(false);
  const [erroModelos, setErroModelos] = useState('');

  const contaCodex = codexAccounts.find((account) => account.id === codexAccount);
  const codexReady = provider !== 'codex' || (!!active && !!contaCodex && contaCodex.auth.status === 'connected' && !codexLoading);
  const contaAgy = agyAccounts.find((account) => account.id === agyAccount);
  // Lista que falhou abre na padrão; a conta que o agy diz estar fora não abre.
  const agyReady = provider !== 'antigravity' || (!agyLoading && !!agyAccount && agyInstalled && contaAgy?.logado !== false);
  const esforcosDoModelo = (value: string) => effortLevels('codex', modelos, value);
  const niveisEsforco = provider === 'codex' ? esforcosDoModelo(modelo)
    : provider === 'claude' && semEsforco(modelo) ? [] : (EFFORT_LEVELS[provider] ?? []);
  const selectedModel = modelos.find((model) => valorModelo(model) === modelo || model.id === modelo);
  const displayModel = modelo ? selectedModel : modelos.find((md) => md.id === catalogDefault || valorModelo(md) === catalogDefault)
    ?? modelos.find((md) => md.id === 'default');
  const codexFastModel = selectedModel ?? displayModel;
  const fastVisible = provider === 'codex' || (provider === 'claude' && !!engine && catalogFast);
  const fastAvailable = !modelsLoading && (provider === 'codex'
    ? codexFastModel?.id !== 'default' && codexFastModel?.service_tiers?.some((tier) => tier.id === 'priority' && tier.hidden !== true) === true
    : fastVisible && (selectedModel ?? displayModel)?.supports_fast === true);
  // Ultra Fast: tier do modelo no Codex (contas Pro); no motor, a conta fixa escolhida precisa ser Pro.
  const ultrafastAvailable = fastAvailable && (provider === 'codex'
    ? codexFastModel?.service_tiers?.some((tier) => tier.id === 'ultrafast' && tier.hidden !== true) === true
    : (selectedModel ?? displayModel)?.supports_ultrafast === true);
  useEffect(() => { if (!fastAvailable) setServiceTier(undefined); }, [fastAvailable]);
  useEffect(() => { if (!ultrafastAvailable) setServiceTier((cur) => cur === 'ultrafast' ? undefined : cur); }, [ultrafastAvailable]);
  const speed = fastAvailable && serviceTier ? serviceTier : 'default';

  useEffect(() => {
    mounted.current = true;
    return () => {
    mounted.current = false;
    pickGeneration.current++;
    codexGeneration.current++;
    archiveGeneration.current++;
    codexController.current?.abort();
    archiveController.current?.abort();
    };
  }, []);

  useEffect(() => {
    setCodexAccounts([]);
    setCodexAccount('');
    setCodexProgress('');
    setRetomando(false);
  }, [provider, active]);

  useEffect(() => {
    if (!enabled) return;
    const generation = ++codexGeneration.current;
    codexController.current?.abort();
    const controller = new AbortController();
    const retry = startCodexRetry();
    codexController.current = controller;
    setCodexError('');
    if (provider !== 'codex' || !active) {
      setCodexLoading(false);
      return () => { retry.cancel(); controller.abort(); codexGeneration.current++; };
    }
    setCodexLoading(true);
    void getCodexAccountsForServer(active, controller.signal)
      .then((accounts) => {
        if (generation !== codexGeneration.current || controller.signal.aborted) return;
        setCodexAccounts(accounts);
        setCodexAccount((current) => accounts.some((account) => account.id === current) ? current
          : defaultCodexAccount(accounts)?.id ?? '');
      })
      .catch((cause: unknown) => {
        if (generation !== codexGeneration.current || controller.signal.aborted) return;
        setCodexError(loadErrorText(cause, m.codex_ui_login_error()));
        retry.failed(cause);
      })
      .finally(() => {
        if (generation === codexGeneration.current && !controller.signal.aborted) setCodexLoading(false);
      });
    return () => { retry.cancel(); controller.abort(); codexGeneration.current++; };
  }, [enabled, provider, active, active?.id, active?.baseUrl, active?.token, codexTry, startCodexRetry]);

  // Retomar uma carga não desfaz escolhas: só a mudança de destino limpa o formulário.
  useEffect(() => {
    setConfigs([]);
    setSelectedConfig(null);
    accountTouched.current = false;
    setAccountChosen(false);
    providerTouched.current = false;
    setSwitchedFrom('');
    setForcedPath('');
    setAgyAccounts([]);
    setAgyAccount('');
    setAgyInstalled(true);
    setMotores({});
    setEngine('');
    setCotas([]);
    setProviderProbe(null);
    // Pela identidade da máquina: renovar o token (novo objeto, mesmo destino) não desfaz o motor escolhido.
  }, [active?.id, active?.baseUrl]);

  useEffect(() => {
    if (!enabled) return;
    if (provider !== 'antigravity') {
      setAgyLoading(false);
      setAgyError('');
      return;
    }
    let alive = true;
    const controller = new AbortController();
    const retry = startAgyRetry();
    setAgyLoading(true);
    setAgyError('');
    // "Ausente" só com a resposta desta máquina; leitura que falha não diz que o agy falta.
    setAgyInstalled(true);
    void getAntigravityAccountsForServer(active, controller.signal)
      .then((r) => {
        if (!alive) return;
        setAgyInstalled(r.instalado);
        setAgyAccounts(r.contas);
        setAgyAccount('default');
      })
      .catch((cause: unknown) => {
        if (!alive) return;
        setAgyError(loadErrorText(cause, m.mobile_antigravity_accounts_error()));
        // Sem a lista, abre na padrão do mesmo jeito.
        setAgyAccount('default');
        retry.failed(cause);
      })
      .finally(() => { if (alive) setAgyLoading(false); });
    return () => { alive = false; retry.cancel(); controller.abort(); };
  }, [enabled, provider, active, active?.id, active?.baseUrl, active?.token, agyTry, startAgyRetry]);

  useEffect(() => {
    if (!enabled) return;
    let alive = true;
    const retry = startConfigsRetry();
    setConfigsError('');
    setConfigsLoading(true);
    void listClaudeConfigsForServer(active)
      .then((cs) => {
        if (!alive) return;
        setConfigs(cs);
        const sel = cs.find((c) => c.active)?.path ?? cs[0]?.path ?? null;
        // Retomar a carga não desfaz a conta que a pessoa já escolheu.
        setSelectedConfig((current) => cs.some((c) => c.path === current) ? current : sel);
      })
      .catch((cause: unknown) => {
        if (!alive) return;
        setConfigsError(loadErrorText(cause, m.criar_sessao_erro()));
        retry.failed(cause);
      })
      .finally(() => { if (alive) setConfigsLoading(false); });
    return () => { alive = false; retry.cancel(); };
  }, [enabled, active, configsTry, startConfigsRetry]);

  useEffect(() => {
    if (!enabled) return;
    let alive = true;
    const controller = new AbortController();
    const retry = startProvidersRetry();
    setProvidersLoading(true);
    setProvidersError('');
    // Sonda falhou: todos ficam oferecidos, e o backend recusa o que não estiver instalado.
    void getProvidersForServer(active, controller.signal)
      .then((probe) => {
        if (!alive) return;
        setProviderProbe(probe);
        const preferred = CREATE_SESSION_PROVIDERS.find((p) => probe[p]?.default && probe[p]?.disponivel);
        if (!providerTouched.current && !initialEngine && preferred && preferred !== provider && (providersTry === 0 || !probe[provider]?.disponivel)) {
          desired.current = { model: '', effort: '' };
          setProvider(preferred);
        }
      })
      .catch((cause: unknown) => {
        if (!alive) return;
        if (!providerTouched.current) setProvidersError(loadErrorText(cause, m.criar_providers_erro()));
        retry.failed(cause);
      })
      .finally(() => { if (alive) setProvidersLoading(false); });
    return () => { alive = false; retry.cancel(); controller.abort(); };
  }, [enabled, active, providersTry, startProvidersRetry]);

  useEffect(() => {
    if (!enabled) return;
    let alive = true;
    void probeServerResponse(active, '/api/cotas')
      .then((response) => {
        if (!response.ok) throw new Error(String(response.status));
        return response.json() as Promise<CotaContaResumo[]>;
      })
      .then((cs) => {
        if (alive) setCotas(cs);
      })
      .catch(() => {});
    return () => { alive = false; };
  }, [enabled, active]);

  // O modo que o servidor vai usar: a tela o mostra e espera por ele, e o pedido não o leva.
  useEffect(() => {
    if (!enabled) return;
    let alive = true;
    const retry = startModeRetry();
    setModeRead({ state: 'loading' });
    // Servidor sem `/api/me` (404) é de antes dos convidados: só o dono chega a ele.
    void getMeForServer(active).catch((cause: { status?: number }) => {
      if (cause?.status === 404) return { role: 'owner' as const };
      throw cause;
    })
      .then(async (me) => {
        if (me.role === 'guest') { if (alive) setModeRead({ state: 'guest' }); return; }
        const value = (await getConfigForServer(active)).campos?.headless_default?.valor;
        if (alive) setModeRead({ state: 'value', value: typeof value === 'boolean' ? value : true });
      })
      .catch((cause: unknown) => {
        if (!alive) return;
        setModeRead({ state: 'error', error: loadErrorText(cause, m.criar_sessao_erro()) });
        retry.failed(cause);
      });
    return () => { alive = false; retry.cancel(); };
  }, [enabled, active, modeTry, startModeRetry]);

  useEffect(() => {
    if (!enabled) return;
    let alive = true;
    const retry = startEnginesRetry();
    setEnginesError('');
    void getEnginesForServer(active)
      .then((r) => {
        if (alive) {
          setMotores(r.motores);
          // Motor pedido que sumiu segura a criação até a pessoa escolher outro provedor ou motor; a semente fica
          // para a próxima leitura (outra máquina, nova tentativa) não liberar a criação sem motor.
          const seed = seededEngine.current;
          if (seed && r.motores[seed]) { seededEngine.current = ''; setEngine(seed); setSeedPending(false); }
          else if (seed) setEnginesError(m.mobile_create_engine_missing());
          else setSeedPending(false);
          if (r.arquivo_corrompido) setEnginesError(m.criar_motores_erro());
        }
      })
      .catch((cause: unknown) => {
        if (!alive) return;
        setEnginesError(loadErrorText(cause, m.criar_motores_erro()));
        retry.failed(cause);
      });
    return () => { alive = false; retry.cancel(); };
  }, [enabled, active, enginesTry, startEnginesRetry]);

  const catalogEngine = provider === 'claude' ? engine || null : null;
  const catalogConfig = provider === 'codex' || provider === 'antigravity' ? null : selectedConfig;
  const catalogAccount = provider === 'codex' ? codexAccount : provider === 'antigravity' ? agyAccount : null;

  useEffect(() => {
    if (!enabled) return;
    const controller = new AbortController();
    let alive = true;
    const previous = previousDefaults.current;
    previousDefaults.current = { key: defaultsKey, engine: catalogEngine };
    if (previous && previous.key !== defaultsKey && (previous.engine || catalogEngine)) {
      desired.current = { model: '', effort: '' };
      choiceTouched.current = false;
    }
    setDefaultsError('');
    setLoadedDefaultsKey(null);
    setPermissao('');
    permissionTouched.current = false;
    const query = [active, provider, catalogEngine, controller.signal] as const;
    const request = engineAccount ? getSessionDefaultsForServer(...query, engineAccount) : getSessionDefaultsForServer(...query);
    void request.then((value) => {
        if (!alive) return;
        if (value && !choiceTouched.current) desired.current = { model: value.model, effort: value.effort };
        if (!permissionTouched.current) setPermissao(value?.permission ?? '');
        setSavedDefault({ key: defaultsKey, value: value ?? null });
        setLoadedDefaultsKey(defaultsKey);
      })
      .catch((cause: unknown) => {
        if (alive) setDefaultsError(loadErrorText(cause, m.criar_sessao_erro()));
      });
    return () => { alive = false; controller.abort(); };
  }, [enabled, active, provider, catalogEngine, engineAccount, defaultsKey, defaultsTry]);

  // modelos quando o destino efetivo do catálogo muda
  useEffect(() => {
    // reset incondicional — igual à PWA (CreateSessionSheet.svelte:141), evita vazar modelo/esforço pro Codex
    setModelo('');
    setModelos([]);
    setCatalogFast(false);
    setCatalogDefault('');
    setServiceTier(undefined);
    setEsforco('');
    setSubagente('');
  }, [provider, catalogEngine, catalogConfig, catalogAccount, engineAccount, active]);

  useEffect(() => {
    if (!enabled) return;
    setModelsLoading(false);
    if (!defaultsReady) return;
    if (provider !== 'claude' && provider !== 'codex' && provider !== 'pi' && provider !== 'kimi' && provider !== 'omp' && provider !== 'antigravity') {
      setModelos([]);
      setListaReduzida(false);
      setErroModelos('');
      return;
    }
    if ((provider === 'codex' || provider === 'antigravity') && (!active || !catalogAccount)) {
      setModelos([]);
      setListaReduzida(false);
      setErroModelos('');
      return;
    }
    let alive = true;
    const controller = new AbortController();
    const retry = startModelsRetry();
    setErroModelos('');
    setModelsLoading(true);
    // Só volta o que a lista nova ainda oferece; o resto abre no padrão.
    const restore = (models: ModelOption[]) => {
      const { model, effort } = desired.current;
      const found = model ? models.find((md) => valorModelo(md) === model || md.id === model) : undefined;
      if (found) setModelo(model);
      const levels = provider === 'codex' ? found?.efforts ?? []
        : provider === 'claude' && semEsforco(model) ? [] : EFFORT_LEVELS[provider] ?? [];
      if (effort && levels.includes(effort)) setEsforco(effort);
    };
    const query = [active, provider, catalogEngine, catalogConfig, catalogAccount, controller.signal] as const;
    const request = engineAccount ? modelOptionsForServer(...query, engineAccount) : modelOptionsForServer(...query);
    void request
      .then((r) => {
        if (!alive) return;
        setModelos(r.models);
        setCatalogFast(r.kind === 'engine' && r.supports_fast === true);
        setCatalogDefault(r.default ?? '');
        setListaReduzida(r.reduced);
        restore(r.models);
      })
      .catch((e) => {
        if (!alive) return;
        setModelos([]);
        setErroModelos(loadErrorText(e, m.criar_modelos_erro()));
        restore([]);
        retry.failed(e);
      })
      .finally(() => { if (alive) setModelsLoading(false); });
    return () => {
      alive = false;
      retry.cancel();
      controller.abort();
    };
  }, [enabled, defaultsReady, provider, catalogEngine, catalogConfig, catalogAccount, engineAccount, active, active?.id, active?.baseUrl, active?.token, modelsTry, startModelsRetry]);

  useEffect(() => {
    if (!enabled) return;
    const generation = ++archiveGeneration.current;
    archiveController.current?.abort();
    const controller = new AbortController();
    archiveController.current = controller;
    setRetomaveis([]);
    setRetomavel('');
    setRetomando(false);
    if (provider !== 'codex' || !picked || !active || !codexAccount) return () => { controller.abort(); archiveGeneration.current++; };
    void getArchivePorCwd(picked, null, 'codex', codexAccount, active, controller.signal)
      .then((entries) => {
        if (generation !== archiveGeneration.current || controller.signal.aborted) return;
        setRetomaveis(entries.filter((entry) => !entry.live));
      })
      .catch(() => {
        if (generation === archiveGeneration.current && !controller.signal.aborted) setRetomaveis([]);
      });
    return () => { controller.abort(); archiveGeneration.current++; };
  }, [enabled, provider, picked, codexAccount, active, active?.id, active?.baseUrl, active?.token]);

  // Conta lembrada (ou a ativa) a 95% ou mais sai sozinha para a de mais folga, a regra do servidor, como no
  // PC e no web; a troca não é gravada como escolha, e motor escolhido não passa por aqui.
  useEffect(() => {
    if (provider !== 'claude' || engine || accountTouched.current || configsLoading) return;
    const target = contaComFolga(selectedConfig, configs, cotas);
    if (!target || target === selectedConfig) return;
    const from = selectedConfig ?? '';
    setSelectedConfig(target);
    setSwitchedFrom(from);
    AccessibilityInfo.announceForAccessibility(m.new_chat_account_switched({ conta: contaLabel(from) }));
  }, [provider, engine, configs, cotas, selectedConfig, configsLoading]);

  const handlePick = useCallback(async (p: string, root?: string) => {
    const generation = ++pickGeneration.current;
    setPicked(p);
    setWorktree(null);
    basePicked.current = null;
    setBranchTaken(false);
    setError('');
    setProjectWarning('');
    if (root) {
      try { rememberProject(active.id, { root, cwd: p }); }
      catch { setProjectWarning(m.criar_projeto_salvar_erro()); }
    }
    setHasSameFolder(false);
    try {
      const sessions = await fetchSessionsForServer(active);
      if (!mounted.current || generation !== pickGeneration.current) return;
      setHasSameFolder(sessions.some((s) => s.cwd === p));
    } catch (cause: unknown) {
      if (!mounted.current || generation !== pickGeneration.current) return;
      setError(cause instanceof Error ? cause.message : m.criar_sessao_erro());
    }
  }, [active]);

  // O formulário abre na última pasta usada nesta máquina, como a conversa nova. Uma vez por montagem:
  // o objeto do servidor muda de identidade (rede local, token) e não pode desfazer a escolha da pessoa.
  const prePicked = useRef(false);
  useEffect(() => {
    if (layout !== 'form' || prePicked.current) return;
    prePicked.current = true;
    const saved = readProject(active.id);
    if (saved) void handlePick(saved.cwd);
  }, [layout, active.id, handlePick]);

  // Uma leitura por abertura, para todos os seletores de pasta desta tela. Só agrupa worktrees: sem
  // ela (convidado, falha) a lista sai plana como antes.
  useEffect(() => {
    if (!enabled) return;
    const controller = new AbortController();
    getWorktreesForServer(active, controller.signal, { sizes: false })
      .then((r) => { if (!controller.signal.aborted) setRepos(r); })
      .catch(() => {});
    return () => controller.abort();
  }, [enabled, active.id]);

  // Android: o voltar do aparelho sai da subtela de pasta antes de fechar o formulário.
  useEffect(() => {
    if (!folderOpen || Platform.OS !== 'android') return;
    const sub = BackHandler.addEventListener('hardwareBackPress', () => { setFolderOpen(false); return true; });
    return () => sub.remove();
  }, [folderOpen]);

  // Nome em branco: o store gera projeto + sufixo da tentativa.
  const canHeadless = canChooseSurface(provider);
  const headlessField = canHeadless && headlessPick !== null ? { headless: headlessPick } : {};
  // Um `manual` salvo como padrão não vale com terminal: a linha mostra o padrão e o pedido não o leva.
  const agyModes = permissionModes('antigravity', canHeadless && headless);
  const agyPermission = agyModes.includes(permissao) ? permissao : '';
  const settings: Omit<NewConversationInput['body'], 'cwd'> = provider === 'codex'
    // Permissão do Codex só vale sem terminal; o corpo só existe com o modo já conhecido.
    ? { provider: 'codex', model: modelo || null, effort: esforco || null, permission_mode: headless ? permissao || null : null, codex_account: codexAccount, ...headlessField }
    : provider === 'antigravity'
    // Só a conta padrão (~/.gemini), que vai sem pasta; o esforço vem no id do modelo.
    ? { provider, config_dir: null, model: modelo || null,
      permission_mode: agyPermission || null, ...headlessField }
    : {
      provider,
      // A conta trocada pela cota também vai: é a que a tela mostra.
      config_dir: provider === 'claude' && (accountChosen || !!switchedFrom) ? selectedConfig : null,
      ...(provider === 'claude' && !engine && forceAccount ? { force_account: true } : {}),
      engine: provider === 'claude' ? engine || null : null,
      ...(engineAccount ? { engine_account: engineAccount, engine_binding: engineConfig?.cliproxy_binding } : {}),
      model: (provider === 'claude' || provider === 'pi' || provider === 'kimi' || provider === 'omp') ? modelo || null : null,
      effort: (provider === 'claude' || provider === 'pi' || provider === 'omp') ? esforco || null : null,
      permission_mode: provider === 'claude' ? permissao || null : null,
      subagent_model: provider === 'claude' && !engine ? subagente || null : null,
      ...headlessField,
    };
  const body = picked && defaultsReady && !(canHeadless && modeUnknown) && !bindingBlocked && !seedPending && !modelsLoading && codexReady && agyReady && !providersLoading && (!providersError || providerTouched.current) ? { ...settings, ...(fastAvailable && serviceTier ? { service_tier: serviceTier } : {}), cwd: picked, remember_provider: true, ...(name.trim() ? { name: name.trim() } : {}), ...(worktree ?? {}) } : null;

  // Reserva pré-aquecida enquanto a tela está aberta, na frente e compondo; o form remonta por
  // máquina, então desmontar também cobre a troca de servidor.
  const reserve = useRef<PrewarmReserve | null>(null);
  reserve.current ??= new PrewarmReserve({ reserve: prewarmSessionForServer, release: releasePrewarmForServer });
  const [foreground, setForeground] = useState(() => AppState.currentState !== 'background');
  useEffect(() => {
    const sub = AppState.addEventListener('change', (next) => setForeground(next !== 'background'));
    return () => sub.remove();
  }, []);
  useEffect(() => () => reserve.current?.release(), []);
  // A reserva só sobe sem terminal; sem escolha, ela leva o modo lido do servidor, o mesmo que a criação vai resolver.
  const reserveBody = foreground && home?.mode !== 'sessions' && body
    ? (headlessPick === null && canHeadless && serverHeadless !== null ? { ...body, headless: serverHeadless } : body) : null;
  const reserveKey = reserveBody ? JSON.stringify(reserveBody) : '';
  useEffect(() => { reserve.current?.update(active, reserveBody); }, [active, reserveKey]);
  const takePrewarm = useCallback(() => reserve.current?.take() ?? null, []);
  const resumePrewarm = useCallback(() => reserve.current?.resume(), []);

  // A escolha de agora no formato do padrão do servidor; a permissão é a que iria no pedido.
  const defaultPermission = provider === 'claude' ? permissao : provider === 'codex' ? (canHeadless && headless ? permissao : '')
    : provider === 'antigravity' ? agyPermission : '';
  // Gravação de um escopo que já saiu não cobre o padrão lido do escopo novo.
  const onDefaultSaved = useCallback((key: string, value: SessionDefaults | null) =>
    setSavedDefault((cur) => (cur && cur.key !== key ? cur : { key, value })), []);
  const defaultChecks = useDefaultChecks({ server: active,
    scope: { key: defaultsKey, provider, engine: catalogEngine, engineAccount: engineAccount ?? null },
    choice: { model: modelo, effort: esforco, permission: defaultPermission },
    saved: savedDefault?.key === defaultsKey ? savedDefault.value : null, onSaved: onDefaultSaved });
  // Como no PC: só com a lista de modelos lida e o padrão do servidor conhecido.
  const defaultChecksShown = !retomavel && defaultsReady && !modelsLoading && !erroModelos && !bindingBlocked;

  // Nada daqui é guardado no aparelho: o padrão da próxima abertura é o do servidor.
  const chooseProvider = (p: Provider) => {
    providerTouched.current = true;
    seededEngine.current = '';
    setSeedPending(false);
    setProvidersError('');
    if (p === provider) return;
    choiceTouched.current = false;
    desired.current = { model: '', effort: '' };
    setProvider(p);
  };
  const chooseModel = (v: string) => {
    choiceTouched.current = true;
    if (!v) setServiceTier(undefined);
    providerTouched.current = true;
    setProvidersError('');
    let effort = esforco;
    if ((provider === 'codex' && !esforcosDoModelo(v).includes(esforco)) || (provider === 'claude' && semEsforco(v))) effort = '';
    desired.current = { model: v, effort };
    setModelo(v);
    setEsforco(effort);
  };
  const chooseEffort = (v: string) => {
    choiceTouched.current = true;
    providerTouched.current = true;
    setProvidersError('');
    desired.current = { ...desired.current, effort: v };
    setEsforco(v);
  };
  const chooseHeadless = (v: boolean) => setHeadlessPick(v);
  const choosePermission = (value: string) => {
    permissionTouched.current = true;
    setPermissao(value);
  };
  const chooseConfig = (v: string) => {
    providerTouched.current = true;
    setProvidersError('');
    accountTouched.current = true;
    setAccountChosen(true);
    setForcedPath('');
    // A escolhida à mão a 95% ou mais também sai para a de mais folga; a linha deixa abrir nela.
    const target = provider === 'claude' && !engine ? contaComFolga(v, configs, cotas) : null;
    setSwitchedFrom(target ? v : '');
    setSelectedConfig(target ?? v);
  };
  const forceSwitched = () => {
    accountTouched.current = true;
    setAccountChosen(true);
    setSelectedConfig(switchedFrom);
    setSwitchedFrom('');
    setForcedPath(switchedFrom);
  };
  const undoForce = () => {
    setForcedPath('');
    const target = contaComFolga(selectedConfig, configs, cotas);
    if (!target || !selectedConfig) return;
    setSwitchedFrom(selectedConfig);
    setSelectedConfig(target);
  };
  const chooseCodexAccount = (value: string) => {
    providerTouched.current = true;
    setProvidersError('');
    codexGeneration.current++;
    archiveGeneration.current++;
    setRetomando(false);
    setRetomavel('');
    setError('');
    setCodexProgress('');
    setCodexAccount(value);
  };

  const handleResume = async (id = retomavel) => {
    const entry = retomaveis.find((candidate) => candidate.session_id === id);
    if (!entry || !active || retomando) return;
    const generation = archiveGeneration.current;
    const codexGenerationAtStart = codexGeneration.current;
    const target = active;
    setRetomando(true);
    setError('');
    try {
      setCodexProgress(m.codex_ui_abrindo_sessao());
      const session = await resumeArchivedConversation(entry.project, entry.session_id, null, null,
        'codex', entry.codex_account ?? codexAccount, target);
      if (!mounted.current || generation !== archiveGeneration.current || codexGenerationAtStart !== codexGeneration.current) return;
      // Na tela inicial não há o que substituir: empilha, senão o gesto de voltar fica sem destino.
      const href = `/s/${target.id}/${session.name}` as never;
      if (router.canGoBack?.() ?? true) router.replace(href);
      else router.push(href);
    } catch (cause: unknown) {
      if (mounted.current && generation === archiveGeneration.current && codexGenerationAtStart === codexGeneration.current) {
        setError(cause instanceof Error ? cause.message : m.criar_sessao_erro());
      }
    } finally {
      if (mounted.current && generation === archiveGeneration.current && codexGenerationAtStart === codexGeneration.current) {
        setRetomando(false);
        setCodexProgress('');
      }
    }
  };

  const destination = enabled ? (
    <View style={styles.field}>
      <Text style={styles.label}>{active.label}</Text>
      <View style={styles.pickerWrap}>
        <CwdPicker server={active} onPick={handlePick} selected={picked} repos={repos} />
      </View>
    </View>
  ) : null;

  const modelLabel = displayModel?.id === 'default' ? m.criar_padrao() : displayModel?.name ?? (modelo || displayModel?.id || m.config_motores_modelo_padrao());
  const modelContext = displayModel?.context ?? (displayModel?.context_length ? ctxWindow(displayModel.context_length).toUpperCase() : '');
  const chipLabel = retomavel ? m.criar_retomar() : [modelLabel, modelContext].filter(Boolean).join(' · ');
  const noProject = picked === NO_PROJECT_CWD;
  const folderLabel = noProject ? m.native_new_chat_no_project() : picked ? basename(picked) : m.native_new_chat_folder();
  const folderRef = useRef<View>(null);
  const branchRef = useRef<View>(null);
  // Branch nova sem nome digitado leva o nome final da sessão, que só existe ao criar.
  // A pasta escolhida pode já ser uma worktree que existe: a pílula diz isso, não "atual".
  const pickedWorktree = !worktree && picked ? repos.flatMap((r) => r.worktrees).find((w) => samePath(w.path, picked)) : undefined;
  const pickedRepo = (picked && folderRepo?.path === picked ? folderRepo.repo : undefined)
    ?? (picked ? repos.flatMap((r) => r.worktrees).find((w) => samePath(w.path, picked))?.repo : undefined);
  const branchLabel = pickedWorktree ? m.worktree_menu_pill_existente({ nome: basename(pickedWorktree.path) })
    : !worktree ? m.native_create_checkout_current()
    : !worktree.new_branch ? m.worktree_menu_pill_nova({ branch: worktree.branch })
    : m.worktree_menu_pill_nova({ branch: worktree.branch || m.worktree_menu_nome_sessao() });
  // O painel desmonta ao fechar e leva junto a releitura depois do fetch; a base padrão ainda precisa acompanhar a remota.
  const newWorktree = !!worktree?.new_branch;
  const gitFolder = pickedRepo ?? picked;
  useEffect(() => {
    if (branchOpen || !newWorktree || !gitFolder || basePicked.current) return;
    let live = true;
    const reread = () => getFolderBranchesForServer(active, gitFolder);
    folderGitActionForServer(active, gitFolder, 'fetch').then(reread, reread)
      .then((r) => {
        const base = freshBase(r);
        // O aviso dos commits de fora mora no painel: fechado, a base só muda se nada ficar de fora.
        if (live && !basePicked.current && !commitsLeftOut(r, base)) setWorktree((v) => (v?.new_branch ? { ...v, base } : v));
      })
      // Sem leitura, fica a base que a tela já mostrou.
      .catch(() => {});
    return () => { live = false; };
  }, [branchOpen, newWorktree, gitFolder, active.id]);

  const on = (yes: boolean) => (yes ? 'on' : 'off') as 'on' | 'off';

  const terminalOnly = m.nova_conversa_so_terminal({ provider: providerName(provider) });
  // A mesma escolha vira pílula na conversa nova e linha no formulário completo.
  const terminalChoice: Choice = {
    icon: 'SquareTerminal',
    label: canHeadless && modeUnknown ? m.comum_carregando() : canHeadless && headless ? m.criar_modo_exec_headless() : m.criar_modo_exec_tmux(),
    aria: m.criar_modo_exec(),
    menuTitle: canHeadless ? m.criar_modo_exec() : terminalOnly,
    actions: [
      {
        id: 'headless', title: m.criar_modo_exec_headless(), state: on(canHeadless && !modeUnknown && headless), attributes: { disabled: !canHeadless },
        subtitle: !canHeadless ? terminalOnly
          : provider === 'codex' ? m.criar_modo_exec_headless_resumo_codex() : m.criar_modo_exec_headless_resumo(),
      },
      {
        id: 'tmux', title: m.criar_modo_exec_tmux(), state: on(!canHeadless || (!modeUnknown && !headless)),
        subtitle: provider === 'codex' ? m.criar_modo_exec_tmux_resumo_codex() : m.criar_modo_exec_tmux_resumo(),
      },
    ],
    onAction: (id) => { if (canHeadless) chooseHeadless(id === 'headless'); },
  };

  const janelaCheia = provider === 'claude' ? janelaEsgotada(cotaSelecionada) : null;
  // A janela que decide a troca (`contaComFolga`): a mais cheia de qualquer tipo, fora as que já viraram.
  const janelaMaisCheia = (path: string) => {
    const agora = Date.now() / 1000;
    const js = cotaDaConta(cotas, path)?.janelas.filter((j) => typeof j.pct === 'number' && isFinite(j.pct)
      && !(typeof j.reset_ts === 'number' && j.reset_ts <= agora)) ?? [];
    return js.reduce<(typeof js)[number] | undefined>((a, j) => (!a || j.pct > a.pct ? j : a), undefined);
  };
  const forcedWindow = provider === 'claude' && !engine && forceAccount && selectedConfig ? janelaMaisCheia(selectedConfig) : undefined;
  const textoJanela = (rotulo: string) => rotulo === '7d' ? m.cota_janela_7d() : rotulo === '5h' ? m.cota_janela_5h() : m.cota_janela_outra({ janela: rotulo });
  // Acima da pílula da conta: a que ficou de fora pela cota, com o botão para abrir nela, ou a forçada, com o desfazer.
  const accountNotice = (() => {
    if (provider !== 'claude' || engine) return null;
    const forced = forceAccount && !!selectedConfig;
    const from = forced ? selectedConfig : switchedFrom;
    const win = from ? janelaMaisCheia(from) : undefined;
    if (!from || !win) return null;
    const args = { conta: contaLabel(from), pct: String(Math.round(win.pct)), janela: textoJanela(win.rotulo) };
    return (
      <View style={styles.accountNote}>
        <Text style={styles.accountNoteText}>
          {forced ? m.new_chat_account_forced_note(args) : m.new_chat_account_force_note({ ...args, destino: contaLabel(selectedConfig ?? '') })}
        </Text>
        <Pressable accessibilityRole="button" hitSlop={8} onPress={forced ? undoForce : forceSwitched}>
          <Text style={styles.accountNoteAction}>{forced ? m.comum_desfazer() : m.new_chat_account_force_action({ conta: args.conta })}</Text>
        </Pressable>
      </View>
    );
  })();
  // Etiqueta da conta pela mesma chave da conta da lista: claude:<pasta>, codex:<home>.
  const activeTags = machineTags[machines.findIndex((sv) => sv.id === active.id)] ?? null;
  const tagGlyph = (key: string) => {
    const tag: AccountBadge | undefined = activeTags?.accounts[key];
    return tag ? <AccountGlyph icon={tag.icon} color={tagHex(tag.color) ?? theme.tokens.text.secondary} provider={provider} size={14} /> : undefined;
  };
  const accountChoice = ((): Choice | null => {
    if (provider !== 'claude' && provider !== 'codex') return null;
    if (proxyEngine) {
      const label = boundAccount ? [boundAccount.email || boundAccount.label, boundAccount.plan].filter(Boolean).join(' · ') : m.cliproxy_no_bound_account();
      return { icon: 'CircleUser', label, aria: m.cliproxy_bound_account(), disabled: true };
    }
    const aria = provider === 'claude' ? m.comum_conta_claude() : m.native_create_codex_account();
    if (provider === 'claude' ? configsLoading : codexLoading) {
      return { icon: 'CircleUser', label: m.comum_carregando(), aria, disabled: true };
    }
    // Falha de carga já aparece nos avisos; aqui fica só o vazio.
    if (provider === 'claude' ? !configs.length : !codexAccounts.length) {
      return (provider === 'claude' ? catalogError : codexError) ? null
        : { icon: 'CircleUser', label: m.native_new_chat_no_accounts(), aria, disabled: true };
    }
    // O estado da cota mora na pílula da conta: esgotada mostra a janela cheia em cor de aviso, e o
    // motivo (ou a troca automática) vem no título do menu, junto da cota de cada conta.
    const label = configs.find((c) => c.path === selectedConfig)?.label ?? m.criar_padrao();
    // A forçada mostra a janela que decidiu a troca; a cheia de antes pode já ter virado.
    const full = forcedWindow ?? (janelaCheia ? cotaSelecionada?.janelas.find((j) => j.rotulo === janelaCheia) : undefined);
    // Forçada: a linha abaixo da pílula já diz o uso; o "no limite" da janela que já virou a contradiria.
    const detail = forcedWindow ? '' : janelaCheia ? m.cota_conta_no_limite({ janela: janelaCheia })
      : switchedFrom ? m.new_chat_account_switched({ conta: contaLabel(switchedFrom) }) : '';
    const alert = !!janelaCheia || !!forcedWindow;
    return provider === 'claude' ? {
      icon: alert ? 'TriangleAlert' : switchedFrom ? 'ArrowLeftRight' : 'CircleUser',
      tone: alert ? 'warning' : switchedFrom ? 'accent' : undefined,
      // Aviso de cota ou troca vence a etiqueta: é estado que a pessoa precisa ver.
      leading: alert || switchedFrom ? undefined : tagGlyph(`claude:${selectedConfig}`),
      label: full ? `${label} · ${full.rotulo} ${Math.round(full.pct)}%` : label,
      aria: detail ? `${aria} (${detail})` : aria,
      menuTitle: detail || aria,
      actions: configs.map((c) => ({
        id: c.path, title: c.label, state: on(c.path === selectedConfig), leading: tagGlyph(`claude:${c.path}`),
        subtitle: [c.active ? (m.switcher_atual() as string) : '', resumoCota(cotaDaConta(cotas, c.path))].filter(Boolean).join(' · ') || undefined,
      })),
      onAction: chooseConfig,
    } : {
      icon: 'CircleUser',
      leading: contaCodex ? tagGlyph(`codex:${contaCodex.home}`) : undefined,
      label: contaCodex?.name ?? m.criar_padrao(),
      aria,
      menuTitle: aria,
      actions: codexAccounts.map((account) => ({
        id: account.id, title: account.name, state: on(account.id === codexAccount), leading: tagGlyph(`codex:${account.home}`),
        subtitle: account.auth.status === 'connected'
          ? (account.auth.email ?? m.codex_ui_account())
          : account.auth.status === 'disconnected' ? m.contas_nao_conectada() : m.codex_ui_unknown(),
      })),
      onAction: chooseCodexAccount,
    };
  })();
  const accountPill = accountChoice ? <QuietPill {...accountChoice} /> : null;
  // Leitura nova falhou e ficaram os números da anterior: sem o motivo, eles pareceriam atuais.
  const cotaAntiga = motivoCotaAntiga(cotaSelecionada);
  // Conta com a janela em 100% não responde: o aviso diz isso em cor de alerta, além dos números.
  const quotaNotice = proxyEngine ? (boundAccount ? <Text style={styles.hint}>
    {[resumoCota(proxyQuota) || m.conta_cota_sem_leitura(), motivoCotaAntiga(proxyQuota)].filter(Boolean).join(' · ')}
  </Text> : null)
    : provider === 'claude' && cotaSelecionada ? (
    <Text style={[styles.hint, janelaCheia ? styles.hintAlerta : null]} accessibilityRole={janelaCheia ? 'alert' : undefined}>
      {janelaCheia ? `${m.cota_conta_no_limite({ janela: janelaCheia })} ` : ''}
      {resumoCota(cotaSelecionada) ||
        `${m.cota_sem_cota()} ${
          cotaSelecionada.estado === 'indisponivel'
            ? (cotaParada(cotaSelecionada) ? m.cota_conta_parada() : '')
            : m.cota_precisa_entrar()
        }`.trim()}
      {cotaAntiga ? ` · ${cotaAntiga}` : ''}
    </Text>
  ) : null;

  const notices = (
    <View style={styles.field}>
      {/* Na conversa nova o aviso fica sob a pasta do título. */}
      {layout === 'form' && hasSameFolder ? <Text style={styles.hint}>{m.criar_ja_existe()}</Text> : null}
      {picked && !codexReady ? (
        codexError ? <Text style={styles.error} accessibilityRole="alert">{codexError}</Text>
          : <Text style={styles.hint}>{codexLoading ? m.comum_carregando() : m.contas_nao_conectada()}</Text>
      ) : null}
      {picked && provider === 'antigravity' && !agyReady ? (
        !agyInstalled ? <Text style={styles.error} accessibilityRole="alert">{m.erro_agy_ausente()}</Text>
          : <Text style={styles.hint}>{agyLoading || !agyAccount ? m.comum_carregando() : m.mobile_antigravity_sign_in_first()}</Text>
      ) : null}
      {provider === 'antigravity' && agyError ? <View>
        <Text style={styles.error} accessibilityRole="alert">{agyError}</Text>
        <Pressable accessibilityRole="button" onPress={() => setAgyTry((n) => n + 1)}>
          <Text style={styles.hint}>{m.native_retry()}</Text>
        </Pressable>
      </View> : null}
      {provider === 'antigravity' && contaAgy && contaAgy.logado !== false && !contaAgy.termos_aceitos
        ? <Text style={styles.hint}>{m.mobile_antigravity_terms_hint()}</Text> : null}
      {projectWarning ? <Text style={styles.error} accessibilityRole="alert">{projectWarning}</Text> : null}
      {error ? <Text style={styles.error} accessibilityRole="alert">{error}</Text> : null}
      {catalogError ? <Text style={styles.error} accessibilityRole="alert">{catalogError}</Text> : null}
      {canHeadless && modeUnknown && modeRead.state === 'error' ? <View>
        <Text style={styles.error} accessibilityRole="alert">{modeRead.error}</Text>
        <Pressable accessibilityRole="button" onPress={() => setModeTry((n) => n + 1)}>
          <Text style={styles.hint}>{m.native_retry()}</Text>
        </Pressable>
      </View> : null}
      {defaultsError ? <View>
        <Text style={styles.error} accessibilityRole="alert">{m.native_create_default_failed({ erro: defaultsError })}</Text>
        <Pressable accessibilityRole="button" onPress={() => setDefaultsTry((n) => n + 1)}>
          <Text style={styles.hint}>{m.native_retry()}</Text>
        </Pressable>
      </View> : null}
      {providersError ? <Text style={styles.error} accessibilityRole="alert">{providersError}</Text> : null}
      {!retomavel && listaReduzida ? <Text style={styles.hint}>{m.criar_lista_reduzida()}</Text> : null}
      {bindingBlocked ? <Text style={styles.error} accessibilityRole="alert">
        {engineConfig?.cliproxy_binding_error || engineConfig?.cliproxy_error || m.cliproxy_choose_connected_account()}
      </Text> : null}
      {!retomavel && !bindingBlocked && erroModelos ? <Text style={styles.hint} accessibilityRole="alert">
        {boundAccount ? erroModelos : m.criar_abre_padrao({ erro: erroModelos } as any)}
      </Text> : null}
      {contextBusy ? <Text style={styles.hint} accessibilityLiveRegion="polite">{m.comum_carregando()}</Text> : null}
      {!contextOpen && context.error ? <Text style={styles.error} accessibilityRole="alert">{context.error}</Text> : null}
      {retomavel ? <Text style={styles.hint}>{m.nova_conversa_retomada_selecionada()}</Text> : null}
      {provider === 'codex' && retomando && codexProgress ? <Text style={styles.hint} accessibilityLiveRegion="polite">{codexProgress}</Text> : null}
    </View>
  );

  const nameField = !retomavel ? (
    <View style={styles.field}>
      <Text style={styles.label}>{m.comum_nome()}</Text>
      <TextInput
        style={styles.input}
        value={name}
        onChangeText={setName}
        placeholder={m.criar_nome_placeholder()}
        accessibilityLabel={m.comum_nome()}
        autoCapitalize="none"
        autoCorrect={false}
        placeholderTextColor={theme.tokens.text.muted}
      />
      <Text style={styles.hintSm}>{m.nova_conversa_nome_automatico()}</Text>
    </View>
  ) : null;
  const codexContextField = provider === 'codex' ? <CodexContextControl control={context} /> : null;
  const engineField = provider === 'claude' && Object.keys(motores).length ? (
    <View style={styles.field}>
      <Text style={styles.label}>{m.comum_motor()}</Text>
      <PanelSelect
        value={engine}
        options={[{ value: '', label: m.criar_claude_sua_conta() }, ...Object.entries(motores).map(([k, v]) => ({ value: k, label: (v as any).label ?? k, hint: (v as any).model }))]}
        onChange={(v) => { choiceTouched.current = false; seededEngine.current = ''; setSeedPending(false); setEngine(v); }}
      />
    </View>
  ) : null;
  const permissionField = provider === 'claude' ? (
    <View style={styles.field}>
      <Text style={styles.label}>{m.criar_permissao()}</Text>
      <PanelSelect
        value={permissao}
        options={[{ value: '', label: m.criar_permissao_padrao() }, ...CLAUDE_PERMISSION_MODES.map((n) => ({ value: n, label: n }))]}
        onChange={choosePermission}
      />
    </View>
  ) : provider === 'antigravity' ? (
    <View style={styles.field}>
      <Text style={styles.label}>{m.criar_permissao()}</Text>
      <PanelSelect
        value={agyPermission}
        options={[{ value: '', label: m.criar_padrao() }, ...agyModes.map((n) => ({ value: n, label: n }))]}
        onChange={choosePermission}
      />
    </View>
  ) : null;
  const subagentField = provider === 'claude' && !engine && modelos.length > 0 && (
    <View style={styles.field}>
      <Text style={styles.label}>{m.criar_subagente()}</Text>
      <PanelSelect
        value={subagente}
        options={[{ value: '', label: m.criar_subagente_padrao() }, ...modelos.filter((md) => md.id !== 'default').map((md) => ({ value: valorModelo(md), label: md.name ?? md.id }))]}
        onChange={(v) => setSubagente(v)}
      />
      <Text style={styles.hintSm}>{m.criar_subagente_ajuda()}</Text>
    </View>
  );
  const resumeField = (
    <>
      {provider === 'codex' && retomaveis.length ? (
        <View style={styles.field}>
          <Text style={styles.label}>{m.criar_retomar()}</Text>
          <PanelSelect
            value={retomavel}
            options={[{ value: '', label: m.criar_retomar_escolha() }, ...retomaveis.map((entry) => ({ value: entry.session_id, label: contextRefsPlainText(entry.ultima || entry.preview || '') || entry.session_id }))]}
            onChange={setRetomavel}
          />
          <Pressable
            onPress={() => void handleResume()}
            disabled={!retomavel || retomando || !codexReady}
            style={[styles.ghostButton, (!retomavel || retomando || !codexReady) && styles.primaryDis]}
          >
            <Text style={styles.ghostTxt}>{retomando ? m.criar_criando() : m.criar_retomar_acao()}</Text>
          </Pressable>
        </View>
      ) : null}

      {provider === 'codex' && retomando && codexProgress ? (
        <View style={styles.rowCenter}>
          <ActivityIndicator />
          <Text style={[styles.hint, { flex: 1 }]} accessibilityLiveRegion="polite">{codexProgress}</Text>
        </View>
      ) : null}
    </>
  );

  const options = (
    <>
      {picked ? <Text style={styles.pickedPath}>{picked}</Text> : null}
      {quotaNotice}
      {nameField}
      {codexContextField}
      {permissionField}
      {subagentField}
      {resumeField}
    </>
  );

  const chooseSpeed = async (tier: Tier) => {
    if (!fastAvailable || (tier === 'ultrafast' && !ultrafastAvailable)) return;
    if (tier !== 'default' && provider === 'codex' && !modelo && codexFastModel) chooseModel(valorModelo(codexFastModel));
    setServiceTier(tier === 'default' ? undefined : tier);
  };

  // Mesmo seletor de branch/worktree nas duas apresentações: linha no formulário, pílula acima da caixa na conversa nova.
  const branchPicker = picked && !noProject ? <BranchPicker server={active} cwd={pickedRepo ?? picked} worktreePath={pickedRepo && pickedRepo !== picked ? picked : undefined}
                              sessionName={name.trim() || basename(picked)}
                              value={worktree} onChange={onWorktree} basePicked={basePicked}
                              onPickFolder={(path, keepOpen, repo) => { setFolderRepo(repo ? { path, repo } : null); void handlePick(path); if (!keepOpen) setBranchOpen(false); }}
                              onTakenChange={setBranchTaken} /> : null;
  const focusBranchAnchor = () => { if (branchRef.current) AccessibilityInfo.sendAccessibilityEvent(branchRef.current, 'focus'); };
  const branchPanel = (
    <AnchoredPanel
      open={branchOpen && !!branchPicker}
      anchor={branchRef.current}
      maxHeightRatio={0.75}
      onClose={() => setBranchOpen(false)}
      onDismissed={focusBranchAnchor}
      label={m.native_create_checkout_branch()}
      closeLabel={m.native_close()}
    >
      {branchPicker}
    </AnchoredPanel>
  );
  // Máquina com a etiqueta dela (ícone, nome e cor), como na lista de sessões.
  const machineView = (sv: Server, i: number) => {
    const tags = machineTags[i];
    const label = machineLabel(sv, tags);
    return { id: sv.id, label, server: sv.label.trim() && sv.label !== label ? sv.label : undefined, color: tagHex(tags?.machine?.color) ?? serverColor(sv.id),
      icon: tags?.machine ? MACHINE_ICON_NAME[tags.machine.icon] : 'Laptop' as const,
      accounts: tags?.accounts, demo: isDemoUrl(`${sv.baseUrl}/`), offline: machineOffline[i] ?? false };
  };
  const activeMachine = machineView(active, machines.findIndex((sv) => sv.id === active.id));
  // A máquina fica sob o título da conversa nova ("em notebook"), com o menu das máquinas e as contas de cada uma.
  const machinePill = (
    <QuietPill label={activeMachine.label} shown={m.native_new_chat_on_machine({ maquina: activeMachine.label })}
      leading={<MachineTile icon={activeMachine.icon} color={activeMachine.color} size={22} />}
      aria={m.native_new_chat_machine()} menuTitle={m.native_new_chat_machine()}
      menu={(close) => <MachineMenu machines={machines.map(machineView)} active={active.id}
        onPick={(id) => { close(); if (id !== active.id) onPickMachine(id); }} />}
      size="lg" />
  );

  // Atalho, não essencial: sem lista ainda (carregando, falha) a linha só não aparece. Só as pastas
  // mudam o resultado, então a lista ao vivo não redesenha a tela a cada evento.
  const lastCwds = useSessions(useShallow((s) => recentCwds(s.byServerRecord?.[active.id], Infinity)));
  const recents = active.invite ? [] : recentProjects(lastCwds, repos, picked, 3);
  const pickRecent = (path: string) => {
    void handlePick(path);
    const generation = pickGeneration.current;
    const current = () => mounted.current && generation === pickGeneration.current;
    // Como a escolha no painel, a pasta fica lembrada com a raiz dela; sem raiz conhecida vale só a escolha.
    getRootsForServer(active).then((roots) => {
      const root = pickFolderRoot(roots, path);
      if (!root || !current()) return;
      try { rememberProject(active.id, { root, cwd: path }); }
      catch { setProjectWarning(m.criar_projeto_salvar_erro()); }
    }, () => { if (current()) setProjectWarning(m.criar_projeto_salvar_erro()); });
  };
  const recentLine = recents.length ? (
    <View style={styles.recents}>
      <Text style={styles.recentsLabel}>{m.native_new_chat_recents()}</Text>
      <View style={styles.recentGrid}>
        {recents.map((path) => (
          <Pressable key={path} onPress={() => pickRecent(path)} style={styles.recentItem}
            accessibilityRole="button" accessibilityLabel={m.native_new_chat_recent_aria({ path })}>
            <Icon name="Folder" size={20} color={theme.tokens.accent.base} />
            {/* Uma linha que encolhe a fonte antes de cortar: nome de pasta não quebra no meio da palavra. */}
            <Text style={styles.recentName} numberOfLines={1} adjustsFontSizeToFit minimumFontScale={theme.base.text.xxxs / theme.base.text.xs} ellipsizeMode="middle">{basename(path)}</Text>
          </Pressable>
        ))}
      </View>
    </View>
  ) : null;

  const blocked = contextBusy || retomando || !!retomavel || (!!picked && !codexReady);
  const branchBlocked = !!(worktree?.new_branch && branchTaken);
  const canCreate = !!body && !blocked && !creating && !branchBlocked;
  // Sem primeira mensagem: cria e abre a conversa vazia, como o formulário do PWA. O nome escolhido vale
  // para a mesma pasta e nome digitado: repetir depois de uma falha de rede não abre uma segunda sessão.
  const createInFlight = useRef(false);
  const attemptName = useRef<{ key: string; name: string; known: boolean } | null>(null);
  const createSession = async () => {
    if (!body || !canCreate || createInFlight.current) return;
    createInFlight.current = true;
    setCreating(true);
    setError('');
    const key = `${body.cwd}\n${name.trim()}`;
    const typed = name.trim();
    const previous = attemptName.current;
    const retry = previous?.key === key;
    // Antes do POST: renovação atrasada subiria outra reserva depois da adoção. O nome da reserva já
    // está guardado para ela no servidor.
    const ticket = takePrewarm();
    let chosen = retry ? previous : typed ? { name: typed, known: true }
      : ticket ? { name: ticket.name, known: true } : await autoSessionName(active, body.cwd);
    // Quem cancelou já saiu: a sessão nasceu e fica na lista, sem puxar a pessoa de volta.
    const open = (session: string) => {
      resetCreatedChat(active.id, session);
      if (mounted.current) router.replace(`/s/${active.id}/${session}` as never);
    };
    try {
      for (let first = true; ; first = false) {
        attemptName.current = { key, ...chosen };
        try {
          const session = await createSessionForServer(active, { ...body, name: chosen.name,
            // Só com o nome da reserva: o digitado, a repetição ou o próximo automático vão sem ela.
            ...(ticket && ticket.name === chosen.name ? { prewarm: ticket.id } : {}),
            ...(body.new_branch && !body.branch?.trim() ? { branch: sanitizeSessionName(chosen.name) } : {}) });
          open(session.name);
          return;
        } catch (cause: unknown) {
          // Nome automático em uso (lista fora do ar, ou outra tela criou no meio): uma tentativa com o próximo.
          if (!typed && !retry && first && (cause as { code?: unknown } | null)?.code === 'erro_nome_em_uso') {
            chosen = await autoSessionName(active, body.cwd, [chosen.name]);
            continue;
          }
          throw cause;
        }
      }
    } catch (cause: unknown) {
      const finalName = chosen.name;
      // A resposta pode ter se perdido com a sessão já criada: a lista do servidor decide. Recusa
      // definitiva (4xx) não criou nada; 409 só conta como "nasceu" na repetição da mesma tentativa, e
      // no nome automático só se ele estava livre na lista: senão a sessão com esse nome pode ser outra.
      const status = (cause as { status?: unknown } | null)?.status;
      const lost = typeof status !== 'number' || status >= 500 || status === 408 || (status === 409 && retry && chosen.known);
      // Recusa respondida encerra a tentativa: o próximo toque escolhe o nome de novo, sem contar como repetição.
      if (!lost) attemptName.current = null;
      const exists = !lost ? false : await fetchSessionsForServer(active)
        .then((list) => list.some((session) => session.name === finalName), () => null);
      const empty = exists && await getHistory(finalName, 1, undefined, 8_000, active)
        .then((events) => events.length === 0, () => false);
      if (empty) open(finalName);
      else if (mounted.current) {
        reserve.current?.resume();
        setError(exists ? m.criar_sessao_recuperacao_nao_vazia({ nome: finalName }) : exists === null ? m.nova_conversa_criacao_incerta() : cause instanceof Error ? cause.message : m.criar_sessao_erro());
      }
    } finally {
      createInFlight.current = false;
      if (mounted.current) setCreating(false);
    }
  };

  if (layout === 'form') {
    const modelActions = [{ id: '', title: m.criar_padrao(), state: on(!modelo) },
      ...modelos.filter((md) => md.id !== 'default').map((md) => ({ id: valorModelo(md), title: md.name ?? md.id, state: on(valorModelo(md) === modelo),
        subtitle: [md.provider, md.context ?? (md.context_length ? ctxWindow(md.context_length).toUpperCase() : '')].filter(Boolean).join(' · ') || undefined }))];
    const accountLabel = provider === 'claude' ? m.comum_conta_claude() : m.native_create_codex_account();
    return (
      <View style={styles.form}>
        <View style={styles.navbar}>
          <Pressable onPress={folderOpen ? () => setFolderOpen(false) : onClose} hitSlop={8} style={[styles.navSide, creating && styles.primaryDis]}
            disabled={creating} accessibilityState={{ disabled: creating }}
            accessibilityRole="button" accessibilityLabel={folderOpen ? m.comum_voltar() : m.comum_cancelar()}>
            {folderOpen ? <Icon name="ChevronLeft" size={22} color={theme.tokens.accent.base} /> : <Text style={styles.navAction}>{m.comum_cancelar()}</Text>}
          </Pressable>
          <Text style={styles.navTitle} accessibilityRole="header" numberOfLines={1}>{folderOpen ? m.native_new_chat_folder() : m.sessao_nova()}</Text>
          {folderOpen ? <View style={styles.navSide} /> : (
            <Pressable onPress={() => void createSession()} disabled={!canCreate} hitSlop={8} style={[styles.navSide, styles.navRight]}
              accessibilityRole="button" accessibilityLabel={m.criar_criar()} accessibilityState={{ disabled: !canCreate, busy: creating }}>
              {creating ? <ActivityIndicator /> : <Text style={[styles.navAction, styles.navStrong, !canCreate && styles.primaryDis]}>{m.criar_criar()}</Text>}
            </Pressable>
          )}
        </View>
        {folderOpen ? (
          <View style={styles.folderScreen}>
            <CwdPicker server={active} onPick={handlePick} selected={picked} autoSelect={false} onDone={() => setFolderOpen(false)} repos={repos} />
          </View>
        ) : (
          <ScrollView contentContainerStyle={styles.formScroll} keyboardShouldPersistTaps="handled" keyboardDismissMode="on-drag">
            {/* No topo: o Criar fica no cabeçalho e a falha dele precisa aparecer sem rolar. */}
            {notices}
            <RowGroup>
              <SettingRow icon="Monitor" label={m.native_new_chat_machine()} value={active.label}
                actions={machines.map((sv) => ({ id: sv.id, title: sv.label, state: on(sv.id === active.id) }))}
                onAction={(id) => { if (id !== active.id) onPickMachine(id); }} />
              <SettingRow ref={folderRef} icon="Folder" label={m.native_new_chat_folder()} detail={picked ?? undefined}
                value={picked ? folderLabel : m.criar_escolher_pasta()} onPress={() => setFolderOpen(true)} />
              {picked && !noProject ? <SettingRow ref={branchRef} icon="GitBranch" label={m.native_create_checkout_branch()} value={branchLabel}
                onPress={() => setBranchOpen(true)} /> : null}
            </RowGroup>
            {/* Os seis agentes à vista, como o seletor do PC: um menu preso à linha cortava os de baixo. */}
            <ProviderTrack value={provider} onChange={chooseProvider} probe={providerProbe} disabled={creating} />
            <RowGroup>
              {accountChoice ? <SettingRow icon={accountChoice.icon ?? 'CircleUser'} label={accountLabel} value={accountChoice.label}
                tone={accountChoice.tone} disabled={accountChoice.disabled} actions={accountChoice.actions}
                menuTitle={accountChoice.menuTitle} onAction={accountChoice.onAction} /> : null}
              {!retomavel ? <SettingRow icon="Sparkles" label={m.composer_modelo()} disabled={modelsLoading}
                value={modelsLoading ? m.comum_carregando() : chipLabel} actions={modelActions} onAction={chooseModel} /> : null}
              {!retomavel && niveisEsforco.length ? <SettingRow icon="Gauge" label={m.composer_esforco()} value={esforco || m.criar_padrao()}
                actions={[{ id: '', title: m.criar_padrao(), state: on(!esforco) }, ...niveisEsforco.map((lv) => ({ id: lv, title: lv, state: on(lv === esforco) }))]}
                onAction={chooseEffort} /> : null}
              {provider === 'claude' ? <SettingRow icon="ShieldCheck" label={m.criar_permissao()} value={permissao || m.criar_padrao()}
                actions={[{ id: '', title: m.criar_permissao_padrao(), state: on(!permissao) }, ...CLAUDE_PERMISSION_MODES.map((n) => ({ id: n, title: n, state: on(n === permissao) }))]}
                onAction={choosePermission} /> : null}
              {provider === 'antigravity' ? <SettingRow icon="ShieldCheck" label={m.criar_permissao()} value={agyPermission || m.criar_padrao()}
                actions={[{ id: '', title: m.criar_padrao(), state: on(!agyPermission) }, ...agyModes.map((n) => ({ id: n, title: n, state: on(n === agyPermission) }))]}
                onAction={choosePermission} /> : null}
              <SettingRow icon="SquareTerminal" label={m.criar_modo_exec()} value={terminalChoice.label} actions={terminalChoice.actions}
                menuTitle={terminalChoice.menuTitle} onAction={terminalChoice.onAction} />
            </RowGroup>
            {nameField}
            {quotaNotice}
            {accountNotice}
            <Pressable accessibilityRole="button" accessibilityState={{ expanded: extrasOpen }}
              onPress={() => setExtrasOpen((value) => !value)} style={styles.more}>
              <Text style={styles.hint}>{m.criar_mais_opcoes()}</Text>
            </Pressable>
            {extrasOpen ? (
              <View style={styles.extras}>
                {codexContextField}
                {engineField}
                {subagentField}
                {fastVisible && !retomavel ? (
                  <View style={styles.group}>
                    <SettingRow icon="Zap" label={m.native_ctl_speed()} disabled={!fastAvailable} value={speedLabel(speed)}
                      menuTitle={m.native_ctl_speed()}
                      actions={SPEED_TIERS.map((tier) => ({ id: tier, title: speedLabel(tier), state: on(speed === tier),
                        subtitle: tier === 'ultrafast' && !ultrafastAvailable ? m.native_ctl_speed_ultrafast_unavailable() : speedHint(tier),
                        attributes: { disabled: tier === 'ultrafast' && !ultrafastAvailable } }))}
                      onAction={(id) => void chooseSpeed(id as Tier)} />
                  </View>
                ) : null}
                {resumeField}
              </View>
            ) : null}
          </ScrollView>
        )}
        {branchPanel}
      </View>
    );
  }

  return (
    <>
    <NewConversation
      home={home}
      server={active}
      destination={destination}
      destinationPending={!picked}
      providerLabel={providerName(provider)}
      folderPanel={(close) => <CwdPicker server={active} onPick={handlePick} selected={picked} autoSelect={false} onDone={close} compact repos={repos} />}
      topPills={(openFolder) => {
        // A pasta mora no título, no meio da tela: "O que vamos fazer em [pasta]?", e a máquina logo abaixo.
        return noProject ? (
          <>
            <Text style={styles.heroText} accessibilityRole="header">{m.native_new_chat_hero()}</Text>
            {hasSameFolder ? <Text style={styles.heroNote}>{m.criar_ja_existe()}</Text> : null}
            <View style={styles.heroMachine}>{machinePill}</View>
            <View style={[styles.headingLine, styles.heroLinkRow]}>
              <Text style={styles.heroHint}>{`${m.native_new_chat_no_project_line()} · `}</Text>
              <Pressable ref={folderRef} onPress={() => openFolder(folderRef.current)} hitSlop={LINK_SLOP} accessibilityRole="button"
                accessibilityLabel={`${m.native_new_chat_folder()}: ${folderLabel}`}>
                <Text style={[styles.headingLink, styles.heroLink]}>{m.native_new_chat_choose_folder()}</Text>
              </Pressable>
            </View>
            {recentLine}
          </>
        ) : (
          <>
            <Text style={styles.heroText} accessibilityRole="header">{m.native_new_chat_hero_in()}</Text>
            <View style={styles.headingLine}>
              <Pressable ref={folderRef} onPress={() => openFolder(folderRef.current)} hitSlop={12} accessibilityRole="button"
                accessibilityLabel={`${m.native_new_chat_folder()}: ${folderLabel}`} style={styles.headingFolder}>
                <Icon name="Folder" size={24} color={theme.tokens.accent.base} />
                <Text style={[styles.heroText, styles.headingFolderText]} numberOfLines={1}>{folderLabel}</Text>
                <Icon name="ChevronDown" size={20} color={theme.tokens.text.muted} />
              </Pressable>
              <Text style={styles.heroText}> ?</Text>
            </View>
            {/* O aviso é sobre a pasta, então mora colado nela, discreto. */}
            {hasSameFolder ? <Text style={styles.heroNote}>{m.criar_ja_existe()}</Text> : null}
            <View style={styles.heroMachine}>{machinePill}</View>
            {!active.invite ? (
              <Pressable onPress={() => { void handlePick(NO_PROJECT_CWD); }} hitSlop={LINK_SLOP} accessibilityRole="button"
                accessibilityHint={m.native_new_chat_no_project_hint()} style={styles.heroLinkRow}>
                <Text style={[styles.headingLink, styles.heroLink]}>{m.native_new_chat_start_without_project()}</Text>
              </Pressable>
            ) : null}
            {recentLine}
          </>
        );
      }}
      optionPills={(
        <>
          {/* Onde o agente trabalha (com projeto), a conta e o terminal, este só com o ícone. */}
          {branchPicker ? <QuietPill ref={branchRef} icon="GitBranch" aria={m.native_create_checkout_branch()}
            label={worktree ? m.native_new_chat_place_new() : pickedWorktree ? branchLabel : m.native_new_chat_place_here()}
            onPress={() => setBranchOpen(true)} /> : null}
          {accountPill}
          {/* Sem o texto, a cor diz o modo: realçado quando a sessão nasce no terminal. */}
          <QuietPill {...terminalChoice} iconOnly tone={canHeadless && headless ? undefined : 'accent'} />
        </>
      )}
      providerChip={(
        <SessionSettingsControl leading={retomavel ? undefined : <ProviderGlyph provider={provider} size={16} inline />} open={settingsOpen} onOpen={() => setSettingsOpen(true)} onClose={() => setSettingsOpen(false)}
          header={<View style={styles.menuHeader}>
            <ProviderLogoTabs value={provider} onChange={chooseProvider} probe={providerProbe} disabled={creating} />
            {engineField ? <View style={styles.menuEngine}>{engineField}</View> : null}
          </View>}
          model={{ display: chipLabel, loading: modelsLoading, notice: erroModelos,
            items: retomavel ? [] : [{ id: '', label: m.criar_padrao(), selected: !modelo },
              // A dica do PC: o id quando o nome é outro, o provedor e a janela.
              ...modelos.filter((md) => md.id !== 'default').map((md) => ({ id: valorModelo(md), label: md.name ?? md.id,
                hint: [md.name && md.name !== md.id ? md.id : '', md.provider, md.context ?? (md.context_length ? ctxWindow(md.context_length).toUpperCase() : '')]
                  .filter(Boolean).join(' · '), selected: valorModelo(md) === modelo }))],
            select: async (it) => chooseModel(it.id ?? '') }}
          effort={{ value: esforco, hidden: !!retomavel || !niveisEsforco.length,
            items: [{ id: '', label: m.criar_padrao(), selected: !esforco }, ...niveisEsforco.map((lv) => ({ label: lv, selected: lv === esforco }))],
            select: async (value) => chooseEffort(value) }}
          fast={{ hidden: !!retomavel || !fastVisible, tier: speed, available: fastAvailable, ultrafast: ultrafastAvailable,
            choose: chooseSpeed }}>
          {defaultChecksShown ? <DefaultChecks control={defaultChecks} providerLabel={providerName(provider)} ready={defaultsReady}
            disabled={creating} /> : null}
          <Pressable accessibilityRole="button" accessibilityState={{ expanded: extrasOpen }}
            onPress={() => setExtrasOpen((value) => !value)} style={styles.more}>
            <Text style={styles.hint}>{m.criar_mais_opcoes()}</Text>
          </Pressable>
          {extrasOpen ? <View style={styles.extras}>{options}</View> : null}
        </SessionSettingsControl>
      )}
      resume={provider === 'codex' && retomaveis.length ? {
        entries: retomaveis.map((entry) => ({ id: entry.session_id, label: contextRefsPlainText(entry.ultima || entry.preview || '') || entry.session_id })),
        busy: retomando,
        onPick: (id) => { void handleResume(id); },
      } : null}
      notices={notices}
      optionNotice={accountNotice}
      body={body}
      takePrewarm={takePrewarm}
      resumePrewarm={resumePrewarm}
      blocked={blocked || branchBlocked}
      commandSource={provider === 'claude' ? {
        // A mesma conta que vai no corpo da criação; sem pasta, a lista traz o que é da conta.
        cwd: picked,
        configDir: accountChosen || !!switchedFrom ? selectedConfig : null,
        headless: canHeadless && headless,
      } : null}
      keyboardOffset={keyboardOffset}
    />
    {branchPanel}
    </>
  );
}

const styles = StyleSheet.create((theme) => ({
  form: { flex: 1 },
  navbar: { minHeight: 48, flexDirection: 'row', alignItems: 'center', paddingHorizontal: theme.base.space[3] },
  navSide: { minWidth: 72, minHeight: 44, justifyContent: 'center' },
  navRight: { alignItems: 'flex-end' },
  navTitle: { flex: 1, textAlign: 'center', fontSize: theme.base.text.base, fontWeight: theme.base.weight.semibold, color: theme.tokens.text.primary },
  navAction: { fontSize: theme.base.text.base, color: theme.tokens.accent.base },
  navStrong: { fontWeight: theme.base.weight.semibold },
  folderScreen: { flex: 1, paddingHorizontal: theme.base.space[3], paddingBottom: theme.base.space[3] },
  formScroll: { gap: theme.base.space[3], paddingHorizontal: theme.base.space[3], paddingBottom: theme.base.space[8] },
  group: { backgroundColor: superficie(theme), borderRadius: theme.base.radius.lg, overflow: 'hidden' },
  sep: { height: StyleSheet.hairlineWidth, marginLeft: theme.base.space[10] + theme.base.space[0.5], backgroundColor: theme.tokens.border.default },
  pickerWrap: { minHeight: 380, flex: 1 },
  more: { minHeight: 36, paddingHorizontal: theme.base.space[2], justifyContent: 'center' },
  menuHeader: { gap: theme.base.space[2], paddingBottom: theme.base.space[1] },
  menuEngine: { paddingHorizontal: theme.base.space[1] },
  extras: { gap: theme.base.space[3], padding: theme.base.space[2] },
  input: {
    flex: 1,
    height: 44,
    backgroundColor: superficie(theme),
    borderWidth: 1,
    borderColor: theme.tokens.border.default,
    borderRadius: theme.base.radius.md,
    color: theme.tokens.text.primary,
    fontSize: theme.base.text.sm,
    paddingHorizontal: theme.base.space[3],
  },
  pickedPath: { fontFamily: theme.base.fontMono, fontSize: theme.base.text.xs, color: theme.tokens.text.muted },
  rowCenter: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2] },
  hint: { fontSize: theme.base.text.sm, color: theme.tokens.text.secondary },
  hintAlerta: { color: theme.tokens.status.warning },
  accountNote: {
    marginHorizontal: theme.base.space[3], marginBottom: theme.base.space[1.5], padding: theme.base.space[2], gap: theme.base.space[1],
    borderRadius: theme.base.radius.md, borderWidth: 1, borderColor: theme.tokens.status.warning, backgroundColor: superficie(theme),
  },
  accountNoteText: { fontSize: theme.base.text.sm, color: theme.tokens.text.secondary },
  accountNoteAction: { fontSize: theme.base.text.sm, fontWeight: theme.base.weight.semibold, color: theme.tokens.accent.base },
  headingLine: { flexDirection: 'row', flexWrap: 'wrap', alignItems: 'center', justifyContent: 'center' },
  heroText: { fontSize: theme.base.text.xxl, lineHeight: 38, fontWeight: theme.base.weight.medium, color: theme.tokens.text.primary, textAlign: 'center' },
  heroHint: { fontSize: theme.base.text.base, color: theme.tokens.text.secondary },
  heroLink: { fontSize: theme.base.text.base },
  // Título, máquina e "sem projeto" em três partes, com o mesmo respiro entre elas.
  heroMachine: { marginTop: theme.base.space[3] },
  heroLinkRow: { marginTop: theme.base.space[2] },
  heroNote: { fontSize: theme.base.text.xs, color: theme.tokens.text.muted, textAlign: 'center', marginTop: theme.base.space[1], paddingHorizontal: theme.base.space[4] },
  // Pastas recentes sob o título: atalhos em grade, ícone e nome, um toque troca a pasta.
  recents: { width: '100%', maxWidth: 420, alignSelf: 'center', gap: theme.base.space[3], marginTop: theme.base.space[6], paddingHorizontal: theme.base.space[4] },
  recentsLabel: { fontSize: theme.base.text.xs, color: theme.tokens.text.muted, textAlign: 'center' },
  recentGrid: { flexDirection: 'row', gap: theme.base.space[3] },
  recentItem: {
    flex: 1, minHeight: 72, alignItems: 'center', justifyContent: 'center', gap: theme.base.space[1.5],
    paddingHorizontal: theme.base.space[2], paddingVertical: theme.base.space[2],
    borderRadius: theme.base.radius.md, borderWidth: 1, borderColor: theme.tokens.border.subtle, backgroundColor: superficie(theme),
  },
  recentName: { fontSize: theme.base.text.xs, color: theme.tokens.text.secondary, textAlign: 'center' },
  // A pasta do título: sublinhada, para se ler como o que dá para trocar.
  headingFolder: {
    flexDirection: 'row', alignItems: 'center', gap: theme.base.space[1.5], maxWidth: '100%',
    borderBottomWidth: 1, borderBottomColor: theme.tokens.accent.dim,
  },
  headingFolderText: { flexShrink: 1, color: theme.tokens.accent.base },
  headingLink: { fontSize: theme.base.text.sm, color: theme.tokens.accent.base },
  hintSm: { fontSize: theme.base.text.xs, color: theme.tokens.text.muted, marginTop: theme.base.space[1] },
  field: { gap: theme.base.space[2] },
  label: { fontSize: theme.base.text.sm, color: theme.tokens.text.secondary, fontWeight: theme.base.weight.medium },
  selectBtn: {
    height: 44,
    backgroundColor: superficie(theme),
    borderWidth: 1,
    borderColor: theme.tokens.border.default,
    borderRadius: theme.base.radius.md,
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'space-between',
    paddingHorizontal: theme.base.space[3],
  },
  selectTxt: { color: theme.tokens.text.primary, fontSize: theme.base.text.sm, flex: 1 },
  selectChevron: { marginLeft: theme.base.space[2] },
  error: { color: theme.tokens.status.error, fontSize: theme.base.text.sm },
  primaryDis: { opacity: 0.5 },
  ghostButton: { height: 44, borderWidth: 1, borderColor: theme.tokens.border.default, borderRadius: theme.base.radius.md, justifyContent: 'center', alignItems: 'center' },
  ghostTxt: { color: theme.tokens.text.secondary, fontSize: theme.base.text.sm },
}));

// Grupo de linhas do formulário: o fio entre elas sai sozinho, também quando uma linha condicional some.
function RowGroup({ children }: { children: ReactNode }) {
  const rows = Children.toArray(children);
  return (
    <View style={styles.group}>
      {rows.map((row, i) => <Fragment key={i}>{i ? <View style={styles.sep} /> : null}{row}</Fragment>)}
    </View>
  );
}
