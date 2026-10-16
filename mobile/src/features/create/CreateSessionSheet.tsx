import { useEffect, useState, useCallback, useRef } from 'react';
import { Icon } from '../../ui/Icon';
import { AccessibilityInfo, ActivityIndicator, Pressable, Text, TextInput, View } from 'react-native';
import { StyleSheet } from 'react-native-unistyles';
import { useRouter } from 'expo-router';
import { getArchivePorCwd, getCodexAccountsForServer, defaultCodexAccount,
  getEnginesForServer, fetchSessionsForServer, listClaudeConfigsForServer, probeServerResponse,
  modelOptionsForServer, resumeArchivedConversation, getProvidersForServer } from '@hangar/core';
import { basename, ctxWindow, providerName, contaComFolga, cotaDaConta, cotaParada, janelaEsgotada, motivoCotaAntiga, resumoCota, CLAUDE_PERMISSION_MODES, EFFORT_LEVELS,
  SESSION_PROVIDERS, hasExecutionMode, effortLevels, contextRefsPlainText } from '@hangar/core';
import type { ArchiveEntry, CodexAccount, ConfigDirInfo, Provider, ModelOption, CotaContaResumo, Server, WorktreeChoice } from '@hangar/core';
import { SessionSettingsControl } from '../../chat/SessionSettings';
import { semEsforco } from '../pills/pills';
import { SettingsList } from '../../ui/SettingsList';
import { useServers } from '../../stores/servers';
import { rememberProject } from '../../stores/createPreferences';
import type { NewConversationInput } from '../../stores/newConversation';
import { CwdPicker } from './CwdPicker';
import { ProviderPicker } from './ProviderPicker';
import { BranchPicker } from './BranchPicker';
import { CodexContextControl, useCodexContextControl } from './CodexContextControl';
import { NewConversation, type HomePresentation } from './NewConversation';
import { QuietPill } from './QuietPill';
import { readChoices, readMachine, rememberChoices, rememberMachine, type NewChatChoices } from './choicesPrefs';
import { superficie } from '../../theme/superficie';
import { AnchoredPanel } from '../../ui/AnchoredPanel';
import * as m from '../../paraglide/messages';

type ProviderProbe = Record<string, { disponivel: boolean; motivo: string | null; default?: boolean }>;

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

export function CreateSessionSheet({ onClose, keyboardOffset, home }: { onClose?: () => void; keyboardOffset?: number; home?: HomePresentation }) {
  const servers = useServers((s) => s.servers);
  const activeId = useServers((s) => s.activeId);
  // A máquina escolhida na pílula vale só para onde a conversa nasce; o servidor ativo é das Configurações.
  const [machine, setMachine] = useState(() => {
    const remembered = readMachine();
    return servers.find((s) => s.id === remembered)?.id ?? servers.find((s) => s.id === activeId)?.id ?? servers[0]?.id ?? null;
  });
  const target = servers.find((s) => s.id === machine) ?? servers.find((s) => s.id === activeId) ?? servers[0];
  // Fixa também o destino que apareceu após a carga das máquinas ou a remoção da escolhida.
  useEffect(() => { if (target && target.id !== machine) setMachine(target.id); }, [machine, target?.id]);
  const pickMachine = useCallback((id: string) => {
    rememberMachine(id);
    setMachine(id);
  }, []);
  return target ? <CreateSessionForm key={`${target.id}:${target.baseUrl}`} active={target} machines={servers} onPickMachine={pickMachine} onClose={onClose} keyboardOffset={keyboardOffset} home={home} />
    : home?.mode === 'sessions' ? <View style={{ flex: 1 }}>{home.sessions}</View>
      : <Text accessibilityRole="alert">{m.servidor_nao_existe()}</Text>;
}

function CreateSessionForm({ active, machines, onPickMachine, keyboardOffset, home }: {
  active: Server; machines: Server[]; onPickMachine: (id: string) => void; onClose?: () => void; keyboardOffset?: number; home?: HomePresentation;
}) {
  const router = useRouter();
  // O form remonta por máquina: estas são as escolhas guardadas para ela.
  const [remembered] = useState(() => readChoices(active.id));
  // O que a pessoa quer de modelo e nível; reaplicado sempre que a lista do provider recarrega.
  const desired = useRef<Required<Pick<NewChatChoices, 'model' | 'effort'>>>({
    model: remembered.model ?? '', effort: remembered.effort ?? '',
  });
  const remember = (patch: NewChatChoices) => rememberChoices(active.id, patch);

  const [picked, setPicked] = useState<string | null>(null);
  const [name, setName] = useState('');
  const [worktree, setWorktree] = useState<WorktreeChoice | null>(null);
  const onWorktree = useCallback((v: WorktreeChoice | null) => setWorktree(v), []);
  const [branchOpen, setBranchOpen] = useState(false);
  const [hasSameFolder, setHasSameFolder] = useState(false);
  const [error, setError] = useState('');
  const [catalogError, setCatalogError] = useState('');
  const [projectWarning, setProjectWarning] = useState('');
  const pickGeneration = useRef(0);

  const [provider, setProvider] = useState<Provider>(() =>
    remembered.provider && SESSION_PROVIDERS.includes(remembered.provider) ? remembered.provider : 'claude');
  // Padrão sem terminal; só Claude e Codex rodam assim.
  const [headless, setHeadless] = useState(remembered.headless ?? true);
  const [providerProbe, setProviderProbe] = useState<ProviderProbe | null>(null);
  const [providersError, setProvidersError] = useState('');
  const providerTouched = useRef(false);
  const [providersLoading, setProvidersLoading] = useState(true);
  const [modelsLoading, setModelsLoading] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [extrasOpen, setExtrasOpen] = useState(false);
  const contextOpen = provider === 'codex' && settingsOpen && extrasOpen;
  const context = useCodexContextControl(active, contextOpen);
  const contextBusy = context.busy;
  const [catalogFast, setCatalogFast] = useState(false);
  const [catalogDefault, setCatalogDefault] = useState('');
  const [serviceTier, setServiceTier] = useState<'priority' | undefined>();

  const [codexAccounts, setCodexAccounts] = useState<CodexAccount[]>([]);
  const [codexAccount, setCodexAccount] = useState('');
  const [codexLoading, setCodexLoading] = useState(false);
  const [codexError, setCodexError] = useState('');
  const [codexProgress, setCodexProgress] = useState('');
  const codexGeneration = useRef(0);
  const codexController = useRef<AbortController | null>(null);
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
  // Conta que a troca automática deixou para trás; a pílula avisa até a próxima escolha.
  const [switchedFrom, setSwitchedFrom] = useState('');
  const [motores, setMotores] = useState<Record<string, { label?: string; model?: string }>>({});
  const [engine, setEngine] = useState('');
  const [modelos, setModelos] = useState<ModelOption[]>([]);
  const [modelo, setModelo] = useState('');
  const [subagente, setSubagente] = useState('');
  const [esforco, setEsforco] = useState('');
  const [permissao, setPermissao] = useState('');
  const [listaReduzida, setListaReduzida] = useState(false);
  const [erroModelos, setErroModelos] = useState('');

  const contaCodex = codexAccounts.find((account) => account.id === codexAccount);
  const codexReady = provider !== 'codex' || (!!active && !!contaCodex && contaCodex.auth.status === 'connected' && !codexLoading);
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
    : fastVisible && selectedModel?.supports_fast === true);
  useEffect(() => { if (!fastAvailable) setServiceTier(undefined); }, [fastAvailable]);

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
    const generation = ++codexGeneration.current;
    codexController.current?.abort();
    const controller = new AbortController();
    codexController.current = controller;
    setCodexAccounts([]);
    setCodexAccount('');
    setCodexError('');
    setCodexProgress('');
    setRetomando(false);
    if (provider !== 'codex' || !active) {
      setCodexLoading(false);
      return () => { controller.abort(); codexGeneration.current++; };
    }
    setCodexLoading(true);
    void getCodexAccountsForServer(active, controller.signal)
      .then((accounts) => {
        if (generation !== codexGeneration.current || controller.signal.aborted) return;
        setCodexAccounts(accounts);
        setCodexAccount(defaultCodexAccount(accounts, remembered.codexAccount)?.id ?? '');
      })
      .catch((cause: unknown) => {
        if (generation !== codexGeneration.current || controller.signal.aborted) return;
        setCodexError(cause instanceof Error ? cause.message : m.codex_ui_login_error());
      })
      .finally(() => {
        if (generation === codexGeneration.current && !controller.signal.aborted) setCodexLoading(false);
      });
    return () => { controller.abort(); codexGeneration.current++; };
  }, [provider, active, active?.id, active?.baseUrl, active?.token]);

  // Cada catálogo pertence ao destino capturado, mesmo após trocar de máquina.
  useEffect(() => {
    let alive = true;
    setConfigs([]);
    setSelectedConfig(null);
    accountTouched.current = false;
    providerTouched.current = false;
    setSwitchedFrom('');
    setMotores({});
    setEngine('');
    setCotas([]);
    setCatalogError('');
    setConfigsLoading(true);
    void listClaudeConfigsForServer(active)
      .then((cs) => {
        if (!alive) return;
        setConfigs(cs);
        const sel = cs.find((c) => c.path === remembered.configDir)?.path ?? cs.find((c) => c.active)?.path ?? cs[0]?.path ?? null;
        setSelectedConfig(sel);
      })
      .catch((cause: unknown) => {
        if (alive) setCatalogError(cause instanceof Error ? cause.message : m.criar_sessao_erro());
      })
      .finally(() => { if (alive) setConfigsLoading(false); });
    setProviderProbe(null);
    setProvidersLoading(true);
    setProvidersError('');
    // Sonda falhou: todos ficam oferecidos, e o backend recusa o que não estiver instalado.
    void getProvidersForServer(active)
      .then((probe) => {
        if (!alive) return;
        setProviderProbe(probe);
        const preferred = SESSION_PROVIDERS.find((p) => probe[p]?.default && probe[p]?.disponivel);
        if (!providerTouched.current && preferred && preferred !== provider) {
          desired.current = { model: '', effort: '' };
          setProvider(preferred);
        }
      })
      .catch((cause: unknown) => {
        if (alive && !providerTouched.current) setProvidersError(cause instanceof Error ? cause.message : m.criar_providers_erro());
      })
      .finally(() => { if (alive) setProvidersLoading(false); });
    void probeServerResponse(active, '/api/cotas')
      .then((response) => {
        if (!response.ok) throw new Error(String(response.status));
        return response.json() as Promise<CotaContaResumo[]>;
      })
      .then((cs) => {
        if (alive) setCotas(cs);
      })
      .catch(() => {});
    void getEnginesForServer(active)
      .then((r) => {
        if (alive) {
          setMotores(r.motores);
          if (r.arquivo_corrompido) setCatalogError(m.criar_motores_erro());
        }
      })
      .catch((cause: unknown) => {
        if (alive) setCatalogError(cause instanceof Error ? cause.message : m.criar_motores_erro());
      });
    return () => {
      alive = false;
    };
  }, [active]);

  const catalogEngine = provider === 'codex' ? null : engine || null;
  const catalogConfig = provider === 'codex' ? null : selectedConfig;
  const catalogAccount = provider === 'codex' ? codexAccount : null;

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
    setModelsLoading(false);
    if (provider !== 'claude' && provider !== 'codex' && provider !== 'pi' && provider !== 'kimi' && provider !== 'omp') {
      setModelos([]);
      setListaReduzida(false);
      setErroModelos('');
      return;
    }
    if (provider === 'codex' && (!active || !catalogAccount)) {
      setModelos([]);
      setListaReduzida(false);
      setErroModelos('');
      return;
    }
    let alive = true;
    const controller = new AbortController();
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
    const request = modelOptionsForServer(active, provider, catalogEngine, catalogConfig, catalogAccount, controller.signal);
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
        setErroModelos(e instanceof Error ? e.message : m.criar_modelos_erro());
        restore([]);
      })
      .finally(() => { if (alive) setModelsLoading(false); });
    return () => {
      alive = false;
      controller.abort();
    };
  }, [provider, catalogEngine, catalogConfig, catalogAccount, active, active?.id, active?.baseUrl, active?.token]);

  useEffect(() => {
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
  }, [provider, picked, codexAccount, active, active?.id, active?.baseUrl, active?.token]);

  // Conta lembrada (ou a ativa) com a janela geral esgotada sai sozinha para a de mais folga, como no PC
  // e no web; a troca não é gravada como escolha, e motor escolhido não passa por aqui.
  useEffect(() => {
    if (provider !== 'claude' || engine || accountTouched.current || configsLoading) return;
    const target = contaComFolga(selectedConfig, configs, cotas, Date.now() / 1000);
    if (!target || target === selectedConfig) return;
    const from = configs.find((c) => c.path === selectedConfig)?.label ?? basename(selectedConfig ?? '');
    setSelectedConfig(target);
    setSwitchedFrom(from);
    AccessibilityInfo.announceForAccessibility(m.new_chat_account_switched({ conta: from }));
  }, [provider, engine, configs, cotas, selectedConfig, configsLoading]);

  const handlePick = useCallback(async (p: string, root?: string) => {
    const generation = ++pickGeneration.current;
    setPicked(p);
    setWorktree(null);
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

  // Nome em branco: o store gera projeto + sufixo da tentativa.
  const canHeadless = hasExecutionMode(provider);
  const settings: Omit<NewConversationInput['body'], 'cwd'> = provider === 'codex'
    ? { provider: 'codex', model: modelo || null, effort: esforco || null, codex_account: codexAccount, headless }
    : {
      provider,
      config_dir: provider === 'claude' ? selectedConfig : null,
      engine: provider === 'claude' ? engine || null : null,
      model: (provider === 'claude' || provider === 'pi' || provider === 'kimi' || provider === 'omp') ? modelo || null : null,
      effort: (provider === 'claude' || provider === 'pi' || provider === 'omp') ? esforco || null : null,
      permission_mode: provider === 'claude' ? permissao || null : null,
      subagent_model: provider === 'claude' && !engine ? subagente || null : null,
      ...(canHeadless ? { headless } : {}),
    };
  const body = picked && codexReady && !providersLoading && (!providersError || providerTouched.current) ? { ...settings, ...(fastAvailable && serviceTier ? { service_tier: serviceTier } : {}), cwd: picked, remember_provider: true, ...(name.trim() ? { name: name.trim() } : {}), ...(worktree ?? {}) } : null;

  // Toda escolha feita aqui volta na próxima abertura desta máquina.
  const chooseProvider = (p: Provider) => {
    providerTouched.current = true;
    setProvidersError('');
    if (p === provider) return;
    desired.current = { model: '', effort: '' };
    setProvider(p);
    remember({ provider: p, model: '', effort: '' });
  };
  const chooseModel = (v: string) => {
    if (!v) setServiceTier(undefined);
    providerTouched.current = true;
    setProvidersError('');
    let effort = esforco;
    if ((provider === 'codex' && !esforcosDoModelo(v).includes(esforco)) || (provider === 'claude' && semEsforco(v))) effort = '';
    desired.current = { model: v, effort };
    setModelo(v);
    setEsforco(effort);
    remember({ model: v, effort });
  };
  const chooseEffort = (v: string) => {
    providerTouched.current = true;
    setProvidersError('');
    desired.current = { ...desired.current, effort: v };
    setEsforco(v);
    remember({ effort: v });
  };
  const chooseHeadless = (v: boolean) => {
    setHeadless(v);
    remember({ headless: v });
  };
  const chooseConfig = (v: string) => {
    providerTouched.current = true;
    setProvidersError('');
    accountTouched.current = true;
    setSwitchedFrom('');
    setSelectedConfig(v);
    remember({ configDir: v });
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
    remember({ codexAccount: value });
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

  const destination = (
    <View style={styles.field}>
      <Text style={styles.label}>{active.label}</Text>
      <View style={styles.pickerWrap}>
        <CwdPicker server={active} onPick={handlePick} selected={picked} />
      </View>
    </View>
  );

  const modelLabel = displayModel?.id === 'default' ? m.criar_padrao() : displayModel?.name ?? (modelo || displayModel?.id || m.config_motores_modelo_padrao());
  const modelContext = displayModel?.context ?? (displayModel?.context_length ? ctxWindow(displayModel.context_length).toUpperCase() : '');
  const chipLabel = retomavel ? m.criar_retomar() : [modelLabel, modelContext].filter(Boolean).join(' · ');
  const folderLabel = picked ? basename(picked) : m.native_new_chat_folder();
  const folderRef = useRef<View>(null);
  const branchRef = useRef<View>(null);
  // Branch nova sem nome digitado leva o nome final da sessão, que só existe ao criar.
  const branchLabel = !worktree ? m.native_create_checkout_current()
    : !worktree.new_branch ? worktree.branch
    : worktree.branch || name.trim() || m.worktree_nova_branch({ base: worktree.base ?? '' });

  const on = (yes: boolean) => (yes ? 'on' : 'off') as 'on' | 'off';

  const terminalOnly = m.nova_conversa_so_terminal({ provider: providerName(provider) });
  const terminalPill = (
    <QuietPill
      icon="SquareTerminal"
      label={canHeadless && headless ? m.criar_modo_exec_headless() : m.criar_modo_exec_tmux()}
      aria={m.criar_modo_exec()}
      menuTitle={canHeadless ? m.criar_modo_exec() : terminalOnly}
      actions={[
        {
          id: 'headless', title: m.criar_modo_exec_headless(), state: on(canHeadless && headless), attributes: { disabled: !canHeadless },
          subtitle: !canHeadless ? terminalOnly
            : provider === 'codex' ? m.criar_modo_exec_headless_resumo_codex() : m.criar_modo_exec_headless_resumo(),
        },
        {
          id: 'tmux', title: m.criar_modo_exec_tmux(), state: on(!canHeadless || !headless),
          subtitle: provider === 'codex' ? m.criar_modo_exec_tmux_resumo_codex() : m.criar_modo_exec_tmux_resumo(),
        },
      ]}
      onAction={(id) => { if (canHeadless) chooseHeadless(id === 'headless'); }}
    />
  );

  const janelaCheia = provider === 'claude' ? janelaEsgotada(cotaSelecionada) : null;
  const accountPill = (() => {
    if (provider !== 'claude' && provider !== 'codex') return null;
    const aria = provider === 'claude' ? m.comum_conta_claude() : m.native_create_codex_account();
    if (provider === 'claude' ? configsLoading : codexLoading) {
      return <QuietPill icon="CircleUser" label={m.comum_carregando()} aria={aria} disabled />;
    }
    // Falha de carga já aparece nos avisos; aqui fica só o vazio.
    if (provider === 'claude' ? !configs.length : !codexAccounts.length) {
      return (provider === 'claude' ? catalogError : codexError) ? null
        : <QuietPill icon="CircleUser" label={m.native_new_chat_no_accounts()} aria={aria} disabled />;
    }
    // O estado da cota mora na pílula da conta: esgotada mostra a janela cheia em cor de aviso, e o
    // motivo (ou a troca automática) vem no título do menu, junto da cota de cada conta.
    const label = configs.find((c) => c.path === selectedConfig)?.label ?? m.criar_padrao();
    const full = janelaCheia ? cotaSelecionada?.janelas.find((j) => j.rotulo === janelaCheia) : undefined;
    const detail = janelaCheia ? m.cota_conta_no_limite({ janela: janelaCheia })
      : switchedFrom ? m.new_chat_account_switched({ conta: switchedFrom }) : '';
    return provider === 'claude' ? (
      <QuietPill
        icon={janelaCheia ? 'TriangleAlert' : switchedFrom ? 'ArrowLeftRight' : 'CircleUser'}
        tone={janelaCheia ? 'warning' : switchedFrom ? 'accent' : undefined}
        label={full ? `${label} · ${full.rotulo} ${Math.round(full.pct)}%` : label}
        aria={detail ? `${aria} (${detail})` : aria}
        menuTitle={detail || aria}
        actions={configs.map((c) => ({
          id: c.path, title: c.label, state: on(c.path === selectedConfig),
          subtitle: [c.active ? (m.switcher_atual() as string) : '', resumoCota(cotaDaConta(cotas, c.path))].filter(Boolean).join(' · ') || undefined,
        }))}
        onAction={chooseConfig}
      />
    ) : (
      <QuietPill
        icon="CircleUser"
        label={contaCodex?.name ?? m.criar_padrao()}
        aria={aria}
        menuTitle={aria}
        actions={codexAccounts.map((account) => ({
          id: account.id, title: account.name, state: on(account.id === codexAccount),
          subtitle: account.auth.status === 'connected'
            ? (account.auth.email ?? m.codex_ui_account())
            : account.auth.status === 'disconnected' ? m.contas_nao_conectada() : m.codex_ui_unknown(),
        }))}
        onAction={chooseCodexAccount}
      />
    );
  })();
  // Leitura nova falhou e ficaram os números da anterior: sem o motivo, eles pareceriam atuais.
  const cotaAntiga = motivoCotaAntiga(cotaSelecionada);
  // Conta com a janela em 100% não responde: o aviso diz isso em cor de alerta, além dos números.
  const quotaNotice = provider === 'claude' && cotaSelecionada ? (
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
      {hasSameFolder ? <Text style={styles.hint}>{m.criar_ja_existe()}</Text> : null}
      {picked && !codexReady ? (
        codexError ? <Text style={styles.error} accessibilityRole="alert">{codexError}</Text>
          : <Text style={styles.hint}>{codexLoading ? m.comum_carregando() : m.contas_nao_conectada()}</Text>
      ) : null}
      {projectWarning ? <Text style={styles.error} accessibilityRole="alert">{projectWarning}</Text> : null}
      {error ? <Text style={styles.error} accessibilityRole="alert">{error}</Text> : null}
      {catalogError ? <Text style={styles.error} accessibilityRole="alert">{catalogError}</Text> : null}
      {providersError ? <Text style={styles.error} accessibilityRole="alert">{providersError}</Text> : null}
      {!retomavel && listaReduzida ? <Text style={styles.hint}>{m.criar_lista_reduzida()}</Text> : null}
      {!retomavel && erroModelos ? <Text style={styles.hint} accessibilityRole="alert">{m.criar_abre_padrao({ erro: erroModelos } as any)}</Text> : null}
      {contextBusy ? <Text style={styles.hint} accessibilityLiveRegion="polite">{m.comum_carregando()}</Text> : null}
      {!contextOpen && context.error ? <Text style={styles.error} accessibilityRole="alert">{context.error}</Text> : null}
      {retomavel ? <Text style={styles.hint}>{m.nova_conversa_retomada_selecionada()}</Text> : null}
      {provider === 'codex' && retomando && codexProgress ? <Text style={styles.hint} accessibilityLiveRegion="polite">{codexProgress}</Text> : null}
    </View>
  );

  const options = (
    <>
      {picked ? <Text style={styles.pickedPath}>{picked}</Text> : null}
      {quotaNotice}
      {!retomavel ? (
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
            placeholderTextColor="#8d8489"
          />
          <Text style={styles.hintSm}>{m.nova_conversa_nome_automatico()}</Text>
        </View>
      ) : null}

      {provider === 'codex' ? <CodexContextControl control={context} /> : null}

      {provider === 'claude' && Object.keys(motores).length ? (
        <View style={styles.field}>
          <Text style={styles.label}>{m.comum_motor()}</Text>
          <PanelSelect
            value={engine}
            options={[{ value: '', label: m.criar_claude_sua_conta() }, ...Object.entries(motores).map(([k, v]) => ({ value: k, label: (v as any).label ?? k, hint: (v as any).model }))]}
            onChange={(v) => setEngine(v)}
          />
        </View>
      ) : null}

      {provider === 'claude' && (
        <View style={styles.field}>
          <Text style={styles.label}>{m.criar_permissao()}</Text>
          <PanelSelect
            value={permissao}
            options={[{ value: '', label: m.criar_permissao_padrao() }, ...CLAUDE_PERMISSION_MODES.map((n) => ({ value: n, label: n }))]}
            onChange={(v) => setPermissao(v)}
          />
        </View>
      )}

      {provider === 'claude' && !engine && modelos.length > 0 && (
        <View style={styles.field}>
          <Text style={styles.label}>{m.criar_subagente()}</Text>
          <PanelSelect
            value={subagente}
            options={[{ value: '', label: m.criar_subagente_padrao() }, ...modelos.filter((md) => md.id !== 'default').map((md) => ({ value: valorModelo(md), label: md.name ?? md.id }))]}
            onChange={(v) => setSubagente(v)}
          />
          <Text style={styles.hintSm}>{m.criar_subagente_ajuda()}</Text>
        </View>
      )}

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

  return (
    <>
    <NewConversation
      home={home}
      server={active}
      destination={destination}
      destinationPending={!picked}
      providerLabel={providerName(provider)}
      folderPanel={(close) => <CwdPicker server={active} onPick={handlePick} selected={picked} autoSelect={false} onDone={close} compact />}
      topPills={(openFolder) => (
        <>
          <QuietPill
            icon="Monitor"
            label={active.label}
            aria={m.native_new_chat_machine()}
            menuTitle={m.native_new_chat_machine()}
            actions={machines.map((s) => ({ id: s.id, title: s.label, state: on(s.id === active.id) }))}
            onAction={(id) => { if (id !== active.id) onPickMachine(id); }}
          />
          <QuietPill ref={folderRef} icon="Folder" label={folderLabel} aria={m.native_new_chat_folder()}
                     onPress={() => openFolder(folderRef.current)} />
          {picked ? (
            <QuietPill ref={branchRef} icon="GitBranch" label={branchLabel}
                       aria={m.native_create_checkout_branch()} onPress={() => setBranchOpen(true)} />
          ) : null}
        </>
      )}
      bottomPills={<>{accountPill}{terminalPill}</>}
      providerChip={(
        <SessionSettingsControl open={settingsOpen} onOpen={() => setSettingsOpen(true)} onClose={() => setSettingsOpen(false)}
          header={<ProviderPicker value={provider} onChange={chooseProvider} disabledMap={providerProbe ?? undefined} />}
          model={{ display: chipLabel, loading: modelsLoading, notice: erroModelos,
            items: retomavel ? [] : [{ id: '', label: m.criar_padrao(), selected: !modelo },
              ...modelos.filter((md) => md.id !== 'default').map((md) => ({ id: valorModelo(md), label: md.name ?? md.id,
                hint: [md.provider, md.context ?? (md.context_length ? ctxWindow(md.context_length).toUpperCase() : '')].filter(Boolean).join(' · '), selected: valorModelo(md) === modelo }))],
            select: async (it) => chooseModel(it.id ?? '') }}
          effort={{ value: esforco, hidden: !!retomavel || !niveisEsforco.length,
            items: [{ id: '', label: m.criar_padrao(), selected: !esforco }, ...niveisEsforco.map((lv) => ({ label: lv, selected: lv === esforco }))],
            select: async (value) => chooseEffort(value) }}
          fast={{ hidden: !!retomavel || !fastVisible, on: fastAvailable && serviceTier === 'priority', available: fastAvailable,
            toggle: async () => {
              if (!fastAvailable) return;
              if (provider === 'codex' && !modelo && codexFastModel) chooseModel(valorModelo(codexFastModel));
              setServiceTier((cur) => cur === 'priority' ? undefined : 'priority');
            } }}>
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
      body={body}
      blocked={contextBusy || retomando || !!retomavel || (!!picked && !codexReady)}
      keyboardOffset={keyboardOffset}
    />
    {/* Mesmo painel preso à pílula que o de pasta abre. */}
    <AnchoredPanel
      open={branchOpen && !!picked}
      anchor={branchRef.current}
      onClose={() => setBranchOpen(false)}
      onDismissed={() => { if (branchRef.current) AccessibilityInfo.sendAccessibilityEvent(branchRef.current, 'focus'); }}
      label={m.native_create_checkout_branch()}
      closeLabel={m.native_close()}
    >
      {picked ? <BranchPicker server={active} cwd={picked} sessionName={name.trim() || basename(picked)}
                              value={worktree} onChange={onWorktree} /> : null}
    </AnchoredPanel>
    </>
  );
}

const styles = StyleSheet.create((theme) => ({
  pickerWrap: { minHeight: 380, flex: 1 },
  more: { minHeight: 36, paddingHorizontal: 8, justifyContent: 'center' },
  extras: { gap: 12, padding: 8 },
  input: {
    flex: 1,
    height: 44,
    backgroundColor: superficie(theme),
    borderWidth: 1,
    borderColor: theme.tokens.border.default,
    borderRadius: theme.base.radius.md,
    color: theme.tokens.text.primary,
    fontSize: 16,
    paddingHorizontal: theme.base.space[3],
  },
  pickedPath: { fontFamily: theme.base.fontMono, fontSize: theme.base.text.xs, color: theme.tokens.text.muted },
  rowCenter: { flexDirection: 'row', alignItems: 'center', gap: 8 },
  hint: { fontSize: theme.base.text.sm, color: theme.tokens.text.secondary },
  hintAlerta: { color: theme.tokens.status.warning },
  hintSm: { fontSize: 13, color: theme.tokens.text.muted, marginTop: 4 },
  field: { gap: theme.base.space[2] },
  label: { fontSize: theme.base.text.sm, color: theme.tokens.text.secondary, fontWeight: '500' },
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
  selectTxt: { color: theme.tokens.text.primary, fontSize: 16, flex: 1 },
  selectChevron: { marginLeft: 8 },
  error: { color: theme.tokens.status.error, fontSize: theme.base.text.sm },
  primaryDis: { opacity: 0.5 },
  ghostButton: { height: 44, borderWidth: 1, borderColor: theme.tokens.border.default, borderRadius: theme.base.radius.md, justifyContent: 'center', alignItems: 'center' },
  ghostTxt: { color: theme.tokens.text.secondary, fontSize: theme.base.text.sm },
}));
