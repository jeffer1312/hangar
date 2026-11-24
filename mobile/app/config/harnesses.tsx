import { useEffect, useRef, useState, type ReactNode } from 'react';
import { ActivityIndicator, Alert, Linking, Pressable, ScrollView, Switch, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import {
  codexAccountMessage, codexOpcoes, getCodexAccountsForServer, getCodexIntegrationForServer, getCodexPreparationForServer,
  getConfigForServer, getHarnessesForServer, getHarnessInstallForServer, patchConfigForServer, prepareCodexAccountForServer,
  repairHarnessForServer, startCodexIntegrationForServer, startHarnessInstallForServer, textoEtapaCodex,
  type CampoConfig, type CodexAccount, type CodexIntegracaoEstado, type CodexOpcoes, type HarnessCli, type HarnessInstall,
  type HarnessItem, type Server,
} from '@hangar/core';
import { Pagina } from '../../src/features/config/Pagina';
import { PageHeader, Pill } from '../../src/features/config/PageHeader';
import { SectionCard } from '../../src/features/config/SectionCard';
import { InfoNotice } from '../../src/features/config/InfoNotice';
import { useSettingsColors } from '../../src/features/config/colors';
import { ProviderGlyph } from '../../src/ui/ProviderGlyph';
import { useServers } from '../../src/stores/servers';
import { prefs } from '../../src/stores/prefs';
import { getLocale } from '../../src/paraglide/runtime';
import * as m from '../../src/paraglide/messages';

const CLAUDE_OPTIONS = [
  { key: 'claude_statusline_update', label: m.harness_claude_statusline_atualizar, help: m.harness_claude_statusline_ajuda },
  { key: 'claude_function_hooks', label: m.harness_claude_function_hooks, help: m.harness_claude_function_hooks_ajuda },
] as const;
const CODEX_SWITCHES = [
  { key: 'codex_sync', label: m.harness_codex_automatica, help: m.harness_codex_automatica_ajuda },
  { key: 'codex_memory_import', label: m.harness_codex_memoria, help: m.harness_codex_memoria_ajuda },
] as const;
const INTEGRATION_POLL_MS = 1500;
const INSTALL_POLL_MS = 1200;

type CodexMessage = CodexIntegracaoEstado['etapa'];
// O tipo do core só tem o que o login usa; esta tela lê a resposta inteira.
interface Integration {
  estado: string;
  etapa?: CodexMessage;
  ultima_execucao?: string | null;
  proxima_atualizacao?: string | null;
  plugins?: { id: string; versao: string; origem: string }[];
  avisos?: CodexMessage[];
  erros?: CodexMessage[];
  confianca_pendente?: boolean;
  progresso?: { passo: number; total: number; sub?: { atual: number; total: number } | null } | null;
  etapa_segundos?: number | null;
  skills?: { ponte: number; nativas: number } | null;
  automatica?: boolean;
  memoria?: boolean;
}
type AccountSync = CodexAccount['sync'];
interface Repair { cli: string; item: string; started: number; outcome?: { ok: boolean; text: string } }

// Chaves montadas a partir de código do servidor: código que o app não conhece volta `undefined`
// e quem chama mostra o código cru, como o desktop.
function tr(key: string, params: Record<string, unknown> = {}): string | undefined {
  const fn = (m as Record<string, unknown>)[key];
  return typeof fn === 'function' ? String((fn as (p: Record<string, unknown>) => unknown)(params)) : undefined;
}
const errorText = (cause: unknown) => (cause instanceof Error && cause.message ? cause.message : m.native_invalid_response());
const statusOf = (cause: unknown) => (cause as { status?: number } | null)?.status;

function itemText(item: HarnessItem): string {
  const params = item.params ?? {};
  const key = item.codigo === 'skills_ok' && 'origem' in params ? 'skills_origem'
    : item.codigo === 'extensoes_outra_fonte' && 'faltam' in params ? 'extensoes_outra_fonte_e_faltam' : item.codigo;
  return tr(`harness_${key}`, params) ?? item.codigo;
}
const ITEM_LABEL_KEY: Record<string, string> = {
  bloco: 'tmux_bloco', default_terminal: 'tmux_term', truecolor: 'tmux_truecolor',
  titulo: 'tmux_titulo', mouse: 'tmux_mouse', persistencia: 'tmux_persist',
};
const itemLabel = (id: string) => tr(`harness_item_${ITEM_LABEL_KEY[id] ?? id}`) ?? id;

// "por quê?" declarado por CLI: o mesmo id em outro card fala de outra coisa.
function explained(item: HarnessItem, cli: string): boolean {
  switch (item.id) {
    case 'credenciais': return ['codex', 'pi', 'omp', 'kimi'].includes(cli) && !!item.conserto?.startsWith('sync:');
    case 'extensoes': return ['pi', 'omp'].includes(cli);
    case 'skills': return ['pi', 'kimi', 'antigravity'].includes(cli);
    case 'hooks': return ['claude', 'kimi', 'antigravity'].includes(cli);
    case 'contas': return cli === 'claude';
    default: return false;
  }
}
const codexText = (msg: CodexMessage) => textoEtapaCodex(msg, (code, params) => tr(`harness_codex_m_${code}`, params));
const installStep = (key?: string | null) => tr(`harness_inst_etapa_${key ?? ''}`) ?? key ?? '';
const english = () => getLocale() === 'en';
function codexDate(value?: string | null): string {
  if (!value) return m.harness_codex_nunca();
  const at = new Date(value);
  return Number.isNaN(at.getTime()) ? value : at.toLocaleString(english() ? 'en-US' : 'pt-BR');
}
const grouped = (n: number) => String(n).replace(/\B(?=(\d{3})+(?!\d))/g, english() ? ',' : '.');
function stepsPercent(p: NonNullable<Integration['progresso']>): number {
  const inside = p.sub && p.sub.total > 0 ? (p.sub.atual - 1) / p.sub.total : 0;
  return p.total ? Math.min(1, Math.max(0, (p.passo - 1 + inside) / p.total)) * 100 : 0;
}

export default function Harnesses() {
  const server = useServers((s) => s.active());
  const { theme } = useUnistyles();
  const c = useSettingsColors();
  const danger = theme.tokens.status.error;

  const [list, setList] = useState<HarnessCli[] | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState('');
  const [repair, setRepair] = useState<Repair | null>(null);
  const [now, setNow] = useState(Date.now());
  const [why, setWhy] = useState<string[]>([]);
  const [install, setInstall] = useState<HarnessInstall | null>(null);
  const [starting, setStarting] = useState<string | null>(null);
  const [installError, setInstallError] = useState<{ owner: string | null; text: string } | null>(null);
  const [options, setOptions] = useState<Record<string, CampoConfig> | null>(null);
  const [optionsError, setOptionsError] = useState('');
  const [toggling, setToggling] = useState<string[]>([]);
  const [integration, setIntegration] = useState<Integration | null>(null);
  const [reconciling, setReconciling] = useState(false);
  const [integrationError, setIntegrationError] = useState('');
  const [integrationToggling, setIntegrationToggling] = useState<string[]>([]);
  const [accounts, setAccounts] = useState<CodexAccount[] | null>(null);
  const [accountsError, setAccountsError] = useState('');
  const [account, setAccount] = useState('default');
  const [accountPreferenceError, setAccountPreferenceError] = useState('');
  const [accountSync, setAccountSync] = useState<AccountSync | null>(null);
  const [accountError, setAccountError] = useState('');
  const [accountReconciling, setAccountReconciling] = useState(false);
  const [codexOptions, setCodexOptions] = useState<CodexOpcoes | null>(null);
  const [codexOptionsToggling, setCodexOptionsToggling] = useState(false);
  const [codexOptionsError, setCodexOptionsError] = useState('');

  // Época: troca de servidor ou saída da tela invalida toda resposta e todo timer em voo.
  const epoch = useRef(0);
  const controller = useRef(new AbortController());
  const seq = useRef({ load: 0, repair: 0, install: 0, options: 0, integration: 0, account: 0, codexOptions: 0 });
  const timers = useRef<Record<string, ReturnType<typeof setTimeout>>>({});
  const installRef = useRef<HarnessInstall | null>(null);
  const startingRef = useRef<string | null>(null);
  const accountRef = useRef('default');
  const codexOptionsRef = useRef<CodexOpcoes | null>(null);
  const logNearBottom = useRef(true);
  const logRef = useRef<ScrollView>(null);

  const schedule = (name: string, ms: number, fn: () => void) => {
    clearTimeout(timers.current[name]);
    timers.current[name] = setTimeout(fn, ms);
  };
  const next = (k: keyof typeof seq.current) => ++seq.current[k];

  const loadList = (s: Server) => {
    const e = epoch.current, n = next('load');
    setLoading(true);
    getHarnessesForServer(s, controller.current.signal).then((value) => {
      if (e !== epoch.current || n !== seq.current.load) return;
      setList(value);
      setError('');
      if (value.some((h) => h.id === 'codex' && h.instalado)) readCodexOptions(s, false);
    }).catch((cause: unknown) => {
      if (e === epoch.current && n === seq.current.load) setError(errorText(cause));
    }).finally(() => { if (e === epoch.current && n === seq.current.load) setLoading(false); });
  };

  const readCodexOptions = (s: Server, clearError: boolean) => {
    const e = epoch.current, n = next('codexOptions');
    if (clearError) setCodexOptionsError('');
    codexOpcoes(s, controller.current.signal).then((value) => {
      if (e !== epoch.current || n !== seq.current.codexOptions) return;
      codexOptionsRef.current = value;
      setCodexOptions(value);
    }).catch((cause: unknown) => {
      if (e !== epoch.current || n !== seq.current.codexOptions) return;
      setCodexOptionsError((old) => old || errorText(cause));
    });
  };

  const toggleCodexOption = (key: 'contexto_estendido', on: boolean) => {
    const current = codexOptionsRef.current;
    if (!server || !current || codexOptionsToggling) return;
    const s = server, e = epoch.current, n = next('codexOptions');
    setCodexOptionsToggling(true);
    setCodexOptionsError('');
    const body = { [key]: on };
    codexOpcoes(s, controller.current.signal, body).then((value) => {
      if (e !== epoch.current) return;
      if (n === seq.current.codexOptions) { codexOptionsRef.current = value; setCodexOptions(value); } else readCodexOptions(s, true);
    }).catch((cause: unknown) => {
      if (e !== epoch.current) return;
      setCodexOptionsError(statusOf(cause) === 409 ? m.erro_codex_opcoes() : errorText(cause));
      readCodexOptions(s, false);
    }).finally(() => { if (e === epoch.current) setCodexOptionsToggling(false); });
  };

  // Sem `write` lê o `/api/config`; com ele grava. Só a resposta mais nova da fila aplica os `campos`.
  const sendOptions = (s: Server, write?: { key: string; on: boolean }) => {
    const e = epoch.current, n = next('options');
    const request = write ? patchConfigForServer(s, { [write.key]: write.on }) : getConfigForServer(s);
    request.then((value) => {
      if (e !== epoch.current) return;
      if (!value.campos) throw new Error(m.native_invalid_response());
      if (n === seq.current.options) setOptions(value.campos);
      else if (write) sendOptions(s);
    }).catch((cause: unknown) => {
      if (e !== epoch.current || (!write && n !== seq.current.options)) return;
      setOptionsError(errorText(cause));
      // A gravação pode ter pegado antes da falha: o servidor relido desempata.
      if (write) sendOptions(s);
    }).finally(() => {
      if (write && e === epoch.current) setToggling((t) => t.filter((k) => k !== write.key));
    });
  };
  const toggleOption = (key: string, on: boolean) => {
    if (!server || toggling.includes(key)) return;
    setOptionsError('');
    setToggling((t) => [...t, key]);
    sendOptions(server, { key, on });
  };

  const readIntegration = (s: Server, reconcile: boolean, clear: boolean) => {
    const e = epoch.current, n = next('integration');
    if (reconcile) setReconciling(true);
    if (clear) setIntegrationError('');
    const request = reconcile ? startCodexIntegrationForServer(s) : getCodexIntegrationForServer(s, controller.current.signal);
    request.then((value) => {
      if (e !== epoch.current || n !== seq.current.integration) return;
      const state = value as unknown as Integration;
      setIntegration(state);
      if (state.estado === 'executando') {
        schedule('integration', INTEGRATION_POLL_MS, () => {
          if (e === epoch.current && n === seq.current.integration) readIntegration(s, false, false);
        });
      }
    }).catch((cause: unknown) => {
      if (e === epoch.current && n === seq.current.integration) setIntegrationError(errorText(cause));
    }).finally(() => { if (e === epoch.current && n === seq.current.integration) setReconciling(false); });
  };

  const toggleIntegration = (key: string, on: boolean) => {
    if (!server || integrationToggling.includes(key)) return;
    const s = server, e = epoch.current;
    setIntegrationError('');
    setIntegrationToggling((t) => [...t, key]);
    patchConfigForServer(s, { [key]: on })
      .catch((cause: unknown) => { if (e === epoch.current) setIntegrationError(errorText(cause)); })
      .finally(() => {
        if (e !== epoch.current) return;
        setIntegrationToggling((t) => t.filter((k) => k !== key));
        // O interruptor mostra o servidor: é a releitura que muda a tela.
        readIntegration(s, false, false);
      });
  };

  const readAccount = (s: Server, id: string, reconcile: boolean) => {
    if (id === 'default') return;
    const e = epoch.current, n = next('account');
    if (reconcile) setAccountReconciling(true);
    setAccountError('');
    const request = reconcile ? prepareCodexAccountForServer(s, id, true) : getCodexPreparationForServer(s, id, controller.current.signal);
    request.then((sync) => {
      if (e !== epoch.current || n !== seq.current.account || id !== accountRef.current) return;
      setAccountSync(sync);
      if (sync.status === 'running') {
        schedule('account', INTEGRATION_POLL_MS, () => {
          if (e === epoch.current && n === seq.current.account) readAccount(s, id, false);
        });
      }
    }).catch((cause: unknown) => {
      if (e === epoch.current && n === seq.current.account && id === accountRef.current) setAccountError(errorText(cause));
    }).finally(() => {
      if (e !== epoch.current) return;
      if (reconcile) setAccountReconciling(false);
      // Reconciliar que voltou depois de outra leitura: relê para não ficar com o estado de antes.
      if (reconcile && n !== seq.current.account && id === accountRef.current) readAccount(s, id, false);
    });
  };

  const loadAccounts = (s: Server) => {
    const e = epoch.current;
    getCodexAccountsForServer(s, controller.current.signal).then((value) => {
      if (e !== epoch.current) return;
      const selected = value.find((a) => a.id === accountRef.current) ?? value.find((a) => a.id === 'default');
      const id = selected?.id ?? 'default';
      const changed = id !== accountRef.current;
      accountRef.current = id;
      setAccount(id);
      setAccounts(value);
      setAccountsError('');
      if (changed) next('account');
      setAccountSync(selected?.sync ?? null);
      if (id !== 'default') readAccount(s, id, false);
    }).catch((cause: unknown) => { if (e === epoch.current) setAccountsError(errorText(cause)); });
  };

  const integrationBusy = reconciling || accountReconciling || (account === 'default'
    ? !integrationError && integration?.estado === 'executando'
    : !accountError && accountSync?.status === 'running');

  const selectAccount = (id: string) => {
    if (!server || integrationBusy || !accounts?.some((a) => a.id === id)) return;
    next('account');
    accountRef.current = id;
    setAccount(id);
    setAccountSync(accounts.find((a) => a.id === id)?.sync ?? null);
    setAccountError('');
    try {
      prefs.set(`harness.codex.account.v1:${server.id}`, id);
      setAccountPreferenceError('');
    } catch {
      setAccountPreferenceError(m.native_harness_codex_preference_error());
    }
    readAccount(server, id, false);
  };
  const reconcile = () => {
    if (!server || integrationBusy) return;
    if (account === 'default') readIntegration(server, true, true); else readAccount(server, account, true);
  };

  // Sem `cli` lê o estado; com ele pede a instalação. A instalação vive no servidor: sair da tela não a perde.
  const pollInstall = (s: Server, cli?: string) => {
    const e = epoch.current, n = next('install');
    if (cli) { setInstallError(null); startingRef.current = cli; setStarting(cli); }
    const request = cli ? startHarnessInstallForServer(s, cli) : getHarnessInstallForServer(s, controller.current.signal);
    request.then((state) => {
      if (e !== epoch.current || n !== seq.current.install) return;
      const running = state.fase === 'rodando';
      if (running) setInstallError(null);
      const finished = installRef.current?.fase === 'rodando' && state.fase === 'pronto';
      installRef.current = state;
      setInstall(state);
      // Quem diz se instalou é a lista relida, não o fim do comando.
      if (running) schedule('install', INSTALL_POLL_MS, () => { if (e === epoch.current) pollInstall(s); });
      else if (finished) loadList(s);
    }).catch((cause: unknown) => {
      if (e !== epoch.current || n !== seq.current.install) return;
      const status = statusOf(cause);
      const text = status === 409 ? m.harness_inst_ocupado()
        : status === 400 ? m.erro_harness_sem_instalador({ cli: cli ?? '' }) : errorText(cause);
      setInstallError({ owner: cli ?? installRef.current?.harness ?? null, text });
      // Resposta perdida não congela a tela: a próxima leitura desempata.
      if (installRef.current?.fase === 'rodando' || cli) schedule('install', INSTALL_POLL_MS, () => { if (e === epoch.current) pollInstall(s); });
    }).finally(() => {
      if (e === epoch.current && n === seq.current.install) { startingRef.current = null; setStarting(null); }
    });
  };

  const confirmInstall = (cli: HarnessCli) => {
    const command = install?.comandos?.[cli.id];
    if (!server || !command) return;
    const s = server;
    Alert.alert(
      m.harness_inst_conf_titulo({ nome: cli.nome }),
      `${m.harness_inst_conf_corpo()}\n\n${command}\n\n${m.harness_inst_conf_depois()}`,
      [
        { text: m.comum_cancelar(), style: 'cancel' },
        { text: m.harness_inst_botao(), onPress: () => {
          // Outra instalação pode ter começado com a confirmação aberta: diz o porquê.
          if (installRef.current?.fase === 'rodando' || startingRef.current) {
            setInstallError({ owner: cli.id, text: m.harness_inst_ocupado() });
            return;
          }
          clearTimeout(timers.current.install);
          pollInstall(s, cli.id);
        } },
      ],
    );
  };

  const runRepair = (id: string, cli: string, item: string) => {
    if (!server || (repair && !repair.outcome)) return;
    const s = server, e = epoch.current, n = next('repair');
    setRepair({ cli, item, started: Date.now() });
    setNow(Date.now());
    setError('');
    repairHarnessForServer(s, id).then((done) => {
      if (e !== epoch.current || n !== seq.current.repair) return;
      // A lista do conserto é mais nova que qualquer leitura em voo.
      next('load');
      setList(done.harnesses);
      setLoading(false);
      setRepair((r) => r && { ...r, outcome: { ok: true, text: done.feito } });
    }).catch((cause: unknown) => {
      if (e !== epoch.current || n !== seq.current.repair) return;
      // O conserto pode ter rodado antes da falha: a lista relida mostra o estado real.
      loadList(s);
      setRepair((r) => r && { ...r, outcome: { ok: false, text: errorText(cause) } });
    });
  };
  const repairRunning = !!repair && !repair.outcome;
  useEffect(() => {
    if (!repairRunning) return;
    const id = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(id);
  }, [repairRunning]);

  const reloadAll = (s: Server) => {
    setOptionsError('');
    setCodexOptionsError('');
    loadList(s);
    pollInstall(s);
    sendOptions(s);
    if (!reconciling) readIntegration(s, false, true);
    loadAccounts(s);
  };

  useEffect(() => {
    const e = ++epoch.current;
    controller.current = new AbortController();
    setList(null); setError(''); setRepair(null); setWhy([]); setInstall(null); installRef.current = null;
    setInstallError(null); setOptions(null); setIntegration(null); setAccounts(null); setAccountsError('');
    let selected = 'default';
    setAccountPreferenceError('');
    if (server) {
      try { selected = prefs.getString(`harness.codex.account.v1:${server.id}`) ?? 'default'; }
      catch { setAccountPreferenceError(m.native_harness_codex_preference_error()); }
    }
    accountRef.current = selected; setAccount(selected); setAccountSync(null); setAccountError('');
    setCodexOptions(null); codexOptionsRef.current = null; setReconciling(false); setAccountReconciling(false);
    if (server) reloadAll(server);
    return () => {
      if (epoch.current === e) epoch.current++;
      controller.current.abort();
      Object.values(timers.current).forEach(clearTimeout);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [server?.id, server?.baseUrl, server?.token]);

  const toggleWhy = (key: string) => setWhy((w) => (w.includes(key) ? w.filter((k) => k !== key) : [...w, key]));

  // ---- pedaços de tela ----
  const line = (text: string, color = c.muted, alert = false, key?: string | number) => (
    <Text key={key} accessibilityRole={alert ? 'alert' : undefined} style={[styles.small, { color }]}>{text}</Text>
  );
  const whyBlock = (key: string, name: string, verdict: string, reason: string) => {
    const open = why.includes(key);
    return (
      <View style={styles.why}>
        <View style={styles.whyHead}>
          <Text style={[styles.small, { color: c.muted, flexShrink: 1 }]}>{verdict}</Text>
          <Pressable
            onPress={() => toggleWhy(key)}
            hitSlop={8}
            accessibilityRole="button"
            accessibilityLabel={m.native_accounts_engine_why_of({ name })}
            accessibilityState={{ expanded: open }}
          >
            <Text style={[styles.small, styles.link, { color: c.accentText }]}>{m.native_accounts_engine_why()}</Text>
          </Pressable>
        </View>
        {open ? line(reason) : null}
      </View>
    );
  };
  const switchRow = (key: string, title: string, help: string, on: boolean, disabled: boolean, onChange: (v: boolean) => void) => (
    <View key={key} style={[styles.switchRow, { borderTopColor: c.border }]}>
      <View style={styles.flex}>
        <Text style={[styles.rowTitle, { color: c.text }]}>{title}</Text>
        <Text style={[styles.small, { color: c.muted }]}>{help}</Text>
      </View>
      <Switch value={on} disabled={disabled} onValueChange={onChange} accessibilityLabel={title} />
    </View>
  );
  const bar = (percent: number, label: string) => (
    <View
      style={[styles.track, { backgroundColor: c.inset }]}
      accessibilityRole="progressbar"
      accessibilityLabel={label}
      accessibilityValue={{ min: 0, max: 100, now: Math.round(percent) }}
    >
      <View style={[styles.fill, { width: `${percent}%`, backgroundColor: c.accent }]} />
    </View>
  );
  const repairLine = (r: Repair, label: string) => (
    <View style={styles.indent}>
      {!r.outcome
        ? line(m.native_harness_fixing({ item: label, s: Math.max(0, Math.floor((now - r.started) / 1000)) }))
        : line(r.outcome.text, r.outcome.ok ? theme.tokens.status.success : danger, !r.outcome.ok)}
    </View>
  );

  const renderItem = (cli: string, item: HarnessItem) => {
    const here = repair && repair.cli === cli && repair.item === item.id ? repair : null;
    const [glyph, color] = item.info ? ['·', c.muted] : item.ok === true ? ['✓', theme.tokens.status.success]
      : item.ok === false ? ['✕', danger] : ['?', c.muted];
    const label = itemLabel(item.id);
    const fix = item.conserto;
    const fixLabel = fix?.startsWith('sync:') ? m.native_harness_sync() : item.ok === false ? m.native_harness_fix() : m.native_harness_redo();
    return (
      <View key={item.id} style={styles.item}>
        <View style={styles.itemRow}>
          <Text style={[styles.glyph, { color }]}>{glyph}</Text>
          <Text style={[styles.itemText, { color: c.muted }]}>
            <Text style={{ color: c.text, fontWeight: theme.base.weight.semibold }}>{label}</Text>{'  '}{itemText(item)}
          </Text>
          {fix ? (
            <Pressable
              disabled={repairRunning}
              onPress={() => runRepair(fix, cli, item.id)}
              accessibilityRole="button"
              accessibilityLabel={`${fixLabel}: ${label}`}
              accessibilityState={{ disabled: repairRunning, busy: !!here && !here.outcome }}
              style={({ pressed }) => [styles.smallButton, { borderColor: c.borderStrong, opacity: repairRunning && !here ? 0.5 : 1 },
                pressed && { backgroundColor: c.hover }]}
            >
              {here && !here.outcome ? <ActivityIndicator size="small" color={c.muted} /> : null}
              <Text style={[styles.smallButtonText, { color: c.text }]}>{fixLabel}</Text>
            </Pressable>
          ) : null}
        </View>
        {explained(item, cli) ? (
          <View style={styles.indent}>
            {whyBlock(`${cli}/${item.id}`, label, tr(`harness_item_${item.id}_vered`) ?? '', tr(`harness_item_${item.id}_porque`) ?? '')}
          </View>
        ) : null}
        {here ? repairLine(here, label) : null}
      </View>
    );
  };

  const claudeOptions = () => {
    if (!options && !optionsError) return null;
    const rows = options ? CLAUDE_OPTIONS.filter((o) => o.key in options) : [];
    // Servidor que não conhece a chave diz o porquê, em vez de a opção sumir calada.
    const old = !!options && !(CLAUDE_OPTIONS[0].key in options);
    return (
      <View>
        {rows.map((o) => switchRow(o.key, o.label(), o.help(), options?.[o.key]?.valor === true, toggling.includes(o.key),
          (v) => toggleOption(o.key, v)))}
        {old || optionsError ? (
          <View style={[styles.block, { borderTopColor: c.border }]}>
            {old ? line(m.harness_opcoes_indisponiveis()) : null}
            {optionsError ? line(optionsError, danger, true) : null}
          </View>
        ) : null}
      </View>
    );
  };

  const accountChoice = () => {
    const many = (accounts?.length ?? 0) > 1;
    const shown = accounts?.length ? accounts : [{ id: 'default', name: '', is_default: true } as CodexAccount];
    const disabled = integrationBusy || !many;
    return (
      <View style={styles.gap4}>
        <Text style={[styles.rowTitle, { color: c.text }]}>{m.codex_ui_account()}</Text>
        {line(!accounts && !accountsError ? m.native_loading() : many ? m.harness_codex_conta_ajuda() : m.harness_codex_conta_so_padrao())}
        <View accessibilityRole="radiogroup" accessibilityLabel={m.codex_ui_account()} style={styles.radios}>
          {shown.map((a) => {
            const on = account === a.id;
            const name = a.is_default ? (a.name ? `${a.name} · ${m.criar_padrao()}` : m.criar_padrao()) : a.name;
            return (
              <Pressable
                key={a.id}
                disabled={disabled}
                onPress={() => selectAccount(a.id)}
                accessibilityRole="radio"
                accessibilityLabel={name}
                accessibilityState={{ checked: on, disabled }}
                style={[styles.radio, { opacity: disabled && !on ? 0.5 : 1 }]}
              >
                <View style={[styles.radioRing, { borderColor: on ? c.accent : c.borderStrong }]}>
                  {on ? <View style={[styles.radioDot, { backgroundColor: c.accent }]} /> : null}
                </View>
                <Text style={[styles.body, { color: c.text }]}>{name}</Text>
              </Pressable>
            );
          })}
        </View>
        {accountsError ? line(m.harness_codex_conta_erro({ erro: accountsError }), danger, true) : null}
        {accountPreferenceError ? line(accountPreferenceError, danger, true) : null}
      </View>
    );
  };

  const accountStatus = () => {
    const chosen = accounts?.find((a) => a.id === account);
    const statuses: Record<string, () => string> = { idle: m.harness_codex_ocioso, running: m.harness_codex_executando,
      ready: m.harness_codex_ok, partial: m.harness_codex_parcial, error: m.harness_codex_erro };
    const status = statuses[accountSync?.status ?? ''] ?? m.harness_codex_indisponivel;
    return (
      <View style={styles.gap3}>
        {chosen?.auth.email ? line(chosen.auth.email) : null}
        {accountSync ? (
          <>
            {line(status(), c.text)}
            {accountSync.trust_pending ? line(m.harness_codex_confianca()) : null}
            {accountSync.issues.map((issue, i) =>
              line(codexAccountMessage(issue), accountSync.status === 'error' ? danger : c.muted, accountSync.status === 'error', i))}
          </>
        ) : null}
        {accountError ? line(accountError, danger, true) : null}
      </View>
    );
  };

  const codexStatus = (state: Integration) => {
    const known = ['ocioso', 'executando', 'ok', 'parcial', 'erro', 'indisponivel'].includes(state.estado);
    const name = known ? tr(`harness_codex_${state.estado}`) ?? state.estado : state.estado;
    const stage = codexText(state.etapa);
    const p = state.estado === 'executando' ? state.progresso : null;
    let progress = '';
    if (p) {
      progress = m.harness_codex_progresso({ passo: p.passo, total: p.total });
      if (p.sub) progress += ` · ${m.harness_codex_progresso_sub({ atual: p.sub.atual, total: p.sub.total })}`;
      if (state.etapa_segundos != null) progress += ` · ${m.harness_codex_etapa_tempo({ s: state.etapa_segundos })}`;
    }
    const plugins = state.plugins ?? [];
    return (
      <View style={styles.gap3}>
        <Text style={[styles.rowTitle, { color: c.text }]}>{stage ? `${name} · ${stage}` : name}</Text>
        {p ? <View style={styles.gap4}>{line(progress)}{bar(stepsPercent(p), m.harness_codex_integracao())}</View> : null}
        {line(m.harness_codex_ultima({ data: codexDate(state.ultima_execucao) }))}
        {state.proxima_atualizacao ? line(m.harness_codex_proxima({ data: codexDate(state.proxima_atualizacao) })) : null}
        {line(m.harness_codex_plugins({ n: plugins.length }))}
        {state.skills ? line(m.harness_codex_skills({ ponte: state.skills.ponte, nativas: state.skills.nativas })) : null}
        {plugins.length ? (
          <View style={styles.plugins}>
            {plugins.map((pl) => (
              <Text key={pl.id} style={[styles.small, { color: c.muted }]}>
                <Text style={{ color: c.text, fontWeight: theme.base.weight.semibold }}>{pl.id}</Text>{` · ${pl.versao} · ${pl.origem}`}
              </Text>
            ))}
          </View>
        ) : null}
        {state.confianca_pendente ? line(m.harness_codex_confianca()) : null}
        {(state.avisos ?? []).map((a, i) => line(codexText(a), c.muted, false, `a${i}`))}
        {(state.erros ?? []).map((f, i) => line(codexText(f), danger, true, `e${i}`))}
      </View>
    );
  };

  const codexIntegration = () => (
    <View style={[styles.block, { borderTopColor: c.border }]}>
      <View style={styles.headRow}>
        <Text style={[styles.rowTitle, styles.flex, { color: c.text }]}>{m.harness_codex_integracao()}</Text>
        <Pressable
          disabled={integrationBusy}
          onPress={reconcile}
          accessibilityRole="button"
          accessibilityState={{ disabled: integrationBusy, busy: integrationBusy }}
          style={({ pressed }) => [styles.smallButton, { borderColor: c.borderStrong }, pressed && { backgroundColor: c.hover }]}
        >
          {integrationBusy ? <ActivityIndicator size="small" color={c.muted} /> : null}
          <Text style={[styles.smallButtonText, { color: c.text }]}>
            {integrationBusy ? m.harness_codex_executando() : m.harness_codex_reconciliar()}
          </Text>
        </Pressable>
      </View>
      {whyBlock('codex/integration-reconcile', m.harness_codex_reconciliar(), m.harness_codex_reconciliar_vered(), m.harness_codex_reconciliar_porque())}
      {accountChoice()}
      {integration ? (
        <>
          {CODEX_SWITCHES.map((o) => switchRow(o.key, o.label(), o.help(),
            !!(o.key === 'codex_sync' ? integration.automatica : integration.memoria),
            integrationToggling.includes(o.key), (v) => toggleIntegration(o.key, v)))}
          {/* O prazo é lido antes de ligar: quem liga achando que já vale é o engano que ele evita. */}
          {whyBlock('codex/integration-memory', m.harness_codex_memoria(), m.harness_codex_memoria_vered(), m.harness_codex_memoria_prazo())}
          {account === 'default' ? codexStatus(integration) : null}
        </>
      ) : null}
      {account !== 'default' ? accountStatus() : null}
      {integrationError ? line(integrationError, danger, true) : null}
    </View>
  );

  const codexOptionsBlock = () => (
    <View style={[styles.block, { borderTopColor: c.border }]}>
      {!codexOptions && !codexOptionsError ? line(m.native_loading()) : null}
      {codexOptions ? (
        <>
          {switchRow('contexto_estendido', m.codex_contexto_titulo(), m.codex_contexto_ajuda(), codexOptions.contexto_estendido,
            codexOptionsToggling, (v) => toggleCodexOption('contexto_estendido', v))}
          <View style={styles.gap4}>
            {line(m.codex_contexto_novas())}
            {line(codexOptions.modelos.length ? m.codex_contexto_limites() : m.codex_contexto_sem_catalogo())}
            {codexOptions.modelos.map((md) => (
              <Text key={md.model} style={[styles.chip, { backgroundColor: c.inset, color: c.muted, fontFamily: theme.base.fontMono }]}>
                {`${md.model}: ${grouped(md.max)}`}
              </Text>
            ))}
            {codexOptions.compactacao != null ? line(m.codex_contexto_compactacao({ n: grouped(codexOptions.compactacao) })) : null}
          </View>
        </>
      ) : null}
      {codexOptionsError ? line(codexOptionsError, danger, true) : null}
    </View>
  );

  // CLI ausente: botão quando o servidor tem comando conferido para este sistema, senão o endereço do fornecedor.
  const installOffer = (h: HarnessCli) => {
    if (!install) return null;
    if (install.comandos?.[h.id]) {
      const busy = install.fase === 'rodando' || !!starting;
      return (
        <View style={styles.itemRow}>
          <Text style={[styles.glyph, { color: c.muted }]}>·</Text>
          <Text style={[styles.itemText, { color: c.muted }]}>{m.harness_inst_disponivel()}</Text>
          <Pressable
            disabled={busy}
            onPress={() => confirmInstall(h)}
            accessibilityRole="button"
            accessibilityLabel={`${m.harness_inst_botao()}: ${h.nome}`}
            accessibilityState={{ disabled: busy, busy: starting === h.id }}
            style={({ pressed }) => [styles.smallButton, { borderColor: c.borderStrong, opacity: busy ? 0.5 : 1 },
              pressed && { backgroundColor: c.hover }]}
          >
            {starting === h.id ? <ActivityIndicator size="small" color={c.muted} /> : null}
            <Text style={[styles.smallButtonText, { color: c.text }]}>{m.harness_inst_botao()}</Text>
          </Pressable>
        </View>
      );
    }
    // Só http(s): o endereço vem do servidor e vira toque que abre o navegador.
    const url = install.manual?.[h.id];
    const safe = url && /^https?:\/\//.test(url) ? url : null;
    return (
      <View style={styles.itemRow}>
        <Text style={[styles.glyph, { color: c.muted }]}>·</Text>
        <View style={[styles.flex, styles.gap3]}>
          <Text style={[styles.body, { color: c.muted }]}>{m.harness_inst_manual()}</Text>
          {safe ? (
            <Pressable onPress={() => { void Linking.openURL(safe); }} accessibilityRole="link" hitSlop={6}>
              <Text style={[styles.small, styles.link, { color: c.accentText }]}>{safe}</Text>
            </Pressable>
          ) : null}
        </View>
      </View>
    );
  };

  const installProgress = (cli: string) => {
    if (!install || install.harness !== cli || (install.fase !== 'rodando' && install.fase !== 'pronto')) return null;
    const running = install.fase === 'rodando';
    const step = installStep(install.etapa);
    const passo = install.passo ?? 0, total = install.total ?? 0;
    const [headline, color] = running
      ? [m.harness_inst_andamento({ passo, total, etapa: step }), c.muted]
      : install.ok === true ? [m.harness_inst_pronto(), theme.tokens.status.success] : [m.harness_inst_falhou({ etapa: step }), danger];
    const log = install.log ?? [];
    return (
      <View style={[styles.indent, styles.gap8]}>
        {line(headline, color, !running && install.ok !== true)}
        {running ? (total > 0 ? bar((passo * 100) / total, m.harness_inst_progresso())
          : <ActivityIndicator accessibilityLabel={m.harness_inst_progresso()} color={c.muted} />) : null}
        {install.erro ? line(install.erro, danger, true) : null}
        {/* Etapa pulada não vive só no log: a manchete promete "ligado ao app". */}
        {install.avisos?.length ? (
          <View style={styles.gap3}>{install.avisos.map((a, i) => line(a, theme.tokens.status.warning, false, i))}</View>
        ) : null}
        {log.length ? (
          <ScrollView
            ref={logRef}
            nestedScrollEnabled
            style={[styles.log, { backgroundColor: c.inset }]}
            accessibilityLabel={m.harness_inst_log()}
            scrollEventThrottle={100}
            onScroll={({ nativeEvent: n }) => {
              logNearBottom.current = n.contentSize.height - n.layoutMeasurement.height - n.contentOffset.y < 40;
            }}
            // Só acompanha a última linha quem já está no fim: quem rolou para cima está lendo.
            onContentSizeChange={() => { if (logNearBottom.current) logRef.current?.scrollToEnd({ animated: false }); }}
          >
            {log.map((l, i) => (
              <Text key={i} style={[styles.logText, { color: c.muted, fontFamily: theme.base.fontMono }]}>{l}</Text>
            ))}
          </ScrollView>
        ) : null}
      </View>
    );
  };

  const renderCard = (h: HarnessCli) => {
    const bad = h.instalado && h.itens.some((i) => i.ok === false);
    const dot = !h.instalado ? c.faint : bad ? danger : theme.tokens.status.success;
    const version = h.instalado ? (h.versao || m.native_harness_installed()) : m.native_harness_not_installed();
    // Desfecho de um item que sumiu depois do conserto (deixou de se aplicar): fica no card.
    const orphan = repair && repair.cli === h.id && repair.outcome && !h.itens.some((i) => i.id === repair.item) ? repair : null;
    const ownError = installError && installError.owner === h.id ? installError.text : null;
    return (
      <SectionCard key={h.id}>
        <View style={[styles.card, !h.instalado && { opacity: 0.7 }]}>
          <View style={styles.cardHead}>
            <View style={[styles.dot, { backgroundColor: dot }]} />
            {h.id === 'tmux' ? (
              <View style={[styles.tmux, { backgroundColor: c.inset }]}><Text style={[styles.tmuxText, { color: c.muted }]}>⌗</Text></View>
            ) : <ProviderGlyph provider={h.id} size={22} />}
            <Text style={[styles.cliName, { color: c.text }]} accessibilityRole="header">{h.nome}</Text>
            <Text style={[styles.version, { color: c.muted, fontFamily: theme.base.fontMono }]} numberOfLines={1}>{version}</Text>
          </View>
          {h.itens.map((i) => renderItem(h.id, i))}
          {orphan ? repairLine(orphan, itemLabel(orphan.item)) : null}
          {h.id === 'claude' ? claudeOptions() : null}
          {h.id === 'codex' ? codexIntegration() : null}
          {h.id === 'codex' && h.instalado ? codexOptionsBlock() : null}
          {!h.instalado ? installOffer(h) : null}
          {ownError ? line(ownError, danger, true) : null}
          {installProgress(h.id)}
        </View>
      </SectionCard>
    );
  };

  const reload = () => { if (server && !loading && !repairRunning) reloadAll(server); };
  const looseInstallError = installError && !(installError.owner && list?.some((h) => h.id === installError.owner))
    ? installError.text : null;

  let body: ReactNode;
  if (!server) body = <InfoNotice text={m.native_settings_offline()} />;
  else if (!list) {
    body = loading ? (
      <View accessibilityRole="progressbar" accessibilityLabel={m.native_loading()} style={styles.loading}>
        <ActivityIndicator color={c.muted} />
        <Text style={[styles.body, { color: c.muted }]}>{m.native_loading()}</Text>
      </View>
    ) : <View style={styles.retry}><Pill icon="RefreshCw" label={m.native_server_retry()} onPress={() => loadList(server)} /></View>;
  } else if (!list.length) body = line(m.native_harness_empty());
  else body = <>{list.map(renderCard)}</>;

  return (
    <Pagina>
      <PageHeader
        title={m.native_settings_page_harnesses()}
        subtitle={m.native_harness_legend()}
        actions={server ? [{ icon: 'RefreshCw', label: m.native_reload(), onPress: reload }] : undefined}
      />
      <View style={styles.scopeRow}>
        <Text style={[styles.scope, { color: c.muted, backgroundColor: c.inset, borderColor: c.border }]}>{m.native_server_scope()}</Text>
        {server ? <Text style={[styles.small, { color: c.faint }]} numberOfLines={1}>{server.label}</Text> : null}
        {list && loading ? <ActivityIndicator size="small" color={c.muted} /> : null}
      </View>
      {error ? <Text accessibilityRole="alert" style={[styles.body, styles.pad, { color: danger }]}>{error}</Text> : null}
      {body}
      {looseInstallError ? <Text accessibilityRole="alert" style={[styles.body, styles.pad, { color: danger }]}>{looseInstallError}</Text> : null}
    </Pagina>
  );
}

const styles = StyleSheet.create((theme) => ({
  flex: { flex: 1, minWidth: 0 },
  pad: { paddingHorizontal: theme.base.space[1] },
  gap3: { gap: theme.base.space[0.5] },
  gap4: { gap: theme.base.space[1] },
  gap8: { gap: theme.base.space[2] },
  body: { fontSize: theme.base.text.sm, lineHeight: 20 },
  small: { fontSize: theme.base.text.xs, lineHeight: 18 },
  link: { textDecorationLine: 'underline' },
  scopeRow: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2], paddingHorizontal: theme.base.space[1] },
  scope: { fontSize: theme.base.text.xs, fontWeight: theme.base.weight.medium, paddingHorizontal: theme.base.space[2], paddingVertical: theme.base.space[0.5], borderRadius: theme.base.radius.sm, borderWidth: 1, overflow: 'hidden' },
  loading: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2], paddingHorizontal: theme.base.space[1] },
  retry: { flexDirection: 'row' },
  card: { paddingHorizontal: theme.base.space[4], paddingVertical: theme.base.space[3], gap: theme.base.space[1] },
  cardHead: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2], paddingBottom: theme.base.space[1] },
  dot: { width: 8, height: 8, borderRadius: theme.base.radius.full },
  tmux: { width: 22, height: 22, borderRadius: theme.base.radius.xs, alignItems: 'center', justifyContent: 'center' },
  tmuxText: { fontSize: theme.base.text.xs, fontWeight: theme.base.weight.bold },
  cliName: { fontSize: theme.base.text.sm, fontWeight: theme.base.weight.semibold },
  version: { flexShrink: 1, fontSize: theme.base.text.xs },
  item: { gap: theme.base.space[0.5] },
  itemRow: { flexDirection: 'row', alignItems: 'flex-start', gap: theme.base.space[2], paddingVertical: theme.base.space[0.5] },
  glyph: { width: 14, fontSize: theme.base.text.sm, fontWeight: theme.base.weight.bold, lineHeight: 20 },
  itemText: { flex: 1, minWidth: 0, fontSize: theme.base.text.sm, lineHeight: 20 },
  indent: { paddingLeft: theme.base.space[5] },
  why: { gap: theme.base.space[1], paddingBottom: theme.base.space[1] },
  whyHead: { flexDirection: 'row', alignItems: 'center', flexWrap: 'wrap', gap: theme.base.space[1.5] },
  smallButton: {
    flexDirection: 'row', alignItems: 'center', gap: theme.base.space[1.5], minHeight: 32, paddingHorizontal: theme.base.space[3],
    borderWidth: 1, borderRadius: theme.base.radius.sm, flexShrink: 0,
  },
  smallButtonText: { fontSize: theme.base.text.xs, fontWeight: theme.base.weight.medium },
  block: { marginTop: theme.base.space[1.5], paddingTop: theme.base.space[2], borderTopWidth: 1, gap: theme.base.space[1.5] },
  headRow: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[3] },
  rowTitle: { fontSize: theme.base.text.sm, fontWeight: theme.base.weight.semibold },
  switchRow: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[3], paddingVertical: theme.base.space[2], borderTopWidth: 1 },
  radios: { gap: theme.base.space[0.5] },
  radio: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2], minHeight: 40 },
  radioRing: { width: 20, height: 20, borderRadius: theme.base.radius.full, borderWidth: 2, alignItems: 'center', justifyContent: 'center' },
  radioDot: { width: 10, height: 10, borderRadius: theme.base.radius.full },
  track: { height: 6, borderRadius: theme.base.radius.full, overflow: 'hidden' },
  fill: { height: 6, borderRadius: theme.base.radius.full },
  plugins: { paddingLeft: theme.base.space[3], gap: theme.base.space[0.5] },
  chip: { alignSelf: 'flex-start', fontSize: theme.base.text.xs, paddingHorizontal: theme.base.space[1.5], paddingVertical: theme.base.space[0.5], borderRadius: theme.base.radius.xs, overflow: 'hidden' },
  log: { maxHeight: 220, borderRadius: theme.base.radius.sm, padding: theme.base.space[2] },
  logText: { fontSize: theme.base.text.xxs, lineHeight: 16 },
}));
