import { useEffect, useRef, useState, type ReactNode } from 'react';
import { ActivityIndicator, AppState, Pressable, ScrollView, Switch, Text, TextInput, View } from 'react-native';
import { KeyboardAvoidingView } from 'react-native-keyboard-controller';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import * as Linking from 'expo-linking';
import { useRouter } from 'expo-router';
import {
  adoptCliproxyForServer, engineCliproxyForServer, getCliproxyLoginForServer, installCliproxyForServer, putCliproxyManagementKeyForServer,
  putEngineForServer, resetCliproxyManagementKeyForServer, sendCliproxyLoginCallbackForServer, startCliproxyLoginForServer,
  type CliproxyAccount, type CliproxyDeteccao, type CliproxyInstallStep, type CliproxyLoginProvider, type CliproxyJob, type Server,
} from '@hangar/core';
import { Sheet } from '../../ui/Sheet';
import { Icon } from '../../ui/Icon';
import { ProviderGlyph } from '../../ui/ProviderGlyph';
import { haptic } from '../../ui/haptics';
import { useSettingsColors } from '../config/colors';
import { InstallItems } from './InstallItems';
import { confirmRestart, failureText, handoverText } from './ProxyUpdate';
import * as m from '../../paraglide/messages';

export { InstallItems };

type Provider = CliproxyLoginProvider;
export type Account = { provider: Provider; email: string; binding?: string | null; prefix?: string | null; plan?: string | null };
type Stage =
  | { k: 'loading' }
  | { k: 'load_error'; error: string }
  // `error`: a entrega ao Hangar que falhou volta às duas escolhas, com o motivo embaixo.
  | { k: 'intro'; external: boolean; error?: string }
  | { k: 'installing'; at: number; adopt?: boolean }
  | { k: 'install_failed'; at: number; error: string }
  | { k: 'pick'; error?: string }
  | { k: 'key'; provider: Provider; error?: string }
  | { k: 'login'; provider: Provider; url: string; state: string }
  | { k: 'connected'; account: Account }
  | { k: 'name'; account: Account }
  | { k: 'done'; account: Account; id: string; name: string; model: string; fast: boolean };

const STEP_AT: Record<CliproxyInstallStep, number> = { download: 0, config: 1, service: 2, start: 3 };
const ENGINE_ID = /^[a-z0-9][a-z0-9_-]{0,31}$/;
// O login devolve para o loopback da máquina do proxy; no celular essa página não abre e o endereço é colado.
const CALLBACK = /^http:\/\/(127\.0\.0\.1|localhost)(:\d+)?\//;

const failure = (e: unknown) => (e ?? {}) as { status?: number; code?: string; envelope?: { msg?: unknown; params?: { sessions?: unknown } } };
const message = (e: unknown) => e instanceof Error ? e.message : m.erro_desconhecido();
const detailOf = (e: unknown) => {
  const msg = failure(e).envelope?.msg;
  return typeof msg === 'string' && msg ? msg : message(e);
};
const providerName = (p: Provider) => p === 'codex' ? m.mobile_cliproxy_wizard_chatgpt() : m.mobile_cliproxy_wizard_claude();
const stepOf = (stage: Stage) => ['pick', 'key', 'login', 'connected'].includes(stage.k) ? 2 : stage.k === 'name' || stage.k === 'done' ? 3 : 1;
// Mesma regra do identificador do EngineEditor, derivada do nome que a pessoa lê.
const engineId = (label: string) => label.toLowerCase().normalize('NFD').replace(/[̀-ͯ]/g, '')
  .replace(/[^a-z0-9_-]+/g, '-').replace(/^[-_]+/, '').slice(0, 32).replace(/-+$/, '');
const defaultName = (a: Account) =>
  `${a.provider === 'codex' ? 'GPT' : m.mobile_cliproxy_wizard_claude()} ${a.email.split('@')[0] ?? ''}`.trim();
// Como no nativo (`allows_fast`): vale o nome depois da última barra, para o modelo com prefixo de conta (`solo/gpt-5.5`).
const fastFor = (model: string) => /^gpt-\d/.test(model.split('/').pop() ?? '');
// Mesma regra do nativo (`default_model`): o primeiro da assinatura (GPT com número, ou Claude); sem ele, o primeiro.
const defaultModel = (models: string[], p: Provider) =>
  models.find(id => p === 'codex' ? fastFor(id) : /claude/i.test(id)) ?? models[0] ?? '';
const accountPrefix = (info: CliproxyDeteccao | null, account: Account) => {
  const current = account.binding ? info?.connected?.find(entry => entry.binding === account.binding && entry.type === account.provider) : undefined;
  return current ? current.prefix : account.prefix;
};
const modelsFor = (info: CliproxyDeteccao | null, account: Account) => {
  const all = (info?.models ?? []).map(x => x.id);
  if (account.provider === 'codex') {
    const found = accountPrefix(info, account);
    if (!found) return [];
    const prefix = `${found}/`;
    return all.filter(id => id.startsWith(prefix)).map(id => id.slice(prefix.length));
  }
  const own = all.filter(id => /^claude/i.test(id));
  return own.length ? own : all;
};

function installError(error: NonNullable<CliproxyJob['error']>): string {
  switch (error.code) {
    case 'cliproxy_download_failed': return m.mobile_cliproxy_wizard_err_download();
    case 'cliproxy_sem_asset': return m.mobile_cliproxy_wizard_err_platform();
    case 'cliproxy_porta_ocupada': return m.mobile_cliproxy_wizard_err_port();
    case 'cliproxy_porta_invalida': return m.mobile_cliproxy_wizard_err_bad_port();
    case 'cliproxy_nao_subiu': return m.mobile_cliproxy_wizard_err_not_started();
    case 'cliproxy_tarefa_caiu': return m.mobile_cliproxy_wizard_err_task_crashed();
    case 'cliproxy_config_manual': case 'cliproxy_config_yaml_invalido': return m.mobile_cliproxy_wizard_err_config();
    case 'cliproxy_servico_falhou': return m.mobile_cliproxy_wizard_err_service({ detail: error.detail });
    default: return m.mobile_cliproxy_wizard_err_generic({ detail: error.detail || error.code });
  }
}

function loginError(e: unknown): string {
  switch (failure(e).code) {
    case 'erro_cliproxy_gerenciamento_desligado': return m.mobile_cliproxy_wizard_err_management_off();
    case 'erro_cliproxy_versao_antiga': return m.mobile_cliproxy_wizard_err_outdated();
    case 'cliproxy_login_porta_ocupada': return m.mobile_cliproxy_wizard_err_login_port();
    case 'cliproxy_login_sem_resposta': case 'cliproxy_login_falhou':
      return m.mobile_cliproxy_wizard_err_login_unreachable({ erro: detailOf(e) });
    default: return message(e);
  }
}

export function CliproxyWizard({ server, names, onSaved, onClose }: {
  server: Server; names: string[]; onSaved: () => void; onClose: () => void;
}) {
  const c = useSettingsColors();
  const { theme } = useUnistyles();
  const insets = useSafeAreaInsets();
  const router = useRouter();
  const [stage, setStage] = useState<Stage>({ k: 'loading' });
  const [info, setInfo] = useState<CliproxyDeteccao | null>(null);
  const [busy, setBusy] = useState(false);
  const [paste, setPaste] = useState('');
  const [key, setKey] = useState('');
  const [loginNote, setLoginNote] = useState('');
  const [name, setName] = useState('');
  const [model, setModel] = useState('');
  const [fast, setFast] = useState(true);
  const [saveError, setSaveError] = useState('');
  // Proxy de fora: passar ao Hangar (o marcado de início) ou usar como está.
  const [adoptPick, setAdoptPick] = useState(true);
  const alive = useRef(true);
  const busyNow = useRef(false);
  const infoNow = useRef<CliproxyDeteccao | null>(null);
  const before = useRef(new Set<string>());
  const pollNow = useRef<() => void>(() => {});
  const posting = useRef(false);
  const stageNow = useRef(stage);
  stageNow.current = stage;
  useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);

  async function refresh() {
    const next = await engineCliproxyForServer(server);
    if (alive.current) { infoNow.current = next; setInfo(next); }
    return next;
  }
  async function run(task: () => Promise<void>) {
    if (busyNow.current || !alive.current) return;
    busyNow.current = true; setBusy(true);
    try { await task(); } finally { busyNow.current = false; if (alive.current) setBusy(false); }
  }

  async function load() {
    setStage({ k: 'loading' });
    try {
      const next = await refresh();
      if (!alive.current) return;
      const setup = next.install;
      // Mesma regra do nativo: proxy respondendo, e instalado pelo Hangar rodando ou sem instalação conhecida.
      const usable = next.found && !!next.base_url && !next.error;
      const ready = usable && (!setup || (setup.state === 'managed' && setup.running) || setup.state === 'absent' || !setup.state);
      if (setup?.job && !setup.job.done) setStage({ k: 'installing', at: STEP_AT[setup.job.step] ?? 0, adopt: setup.job.kind === 'adopt' });
      else if (ready) setStage({ k: 'pick' });
      else setStage({ k: 'intro', external: setup?.state === 'external' });
    } catch (e) {
      if (alive.current) setStage({ k: 'load_error', error: m.mobile_cliproxy_wizard_load_failed({ erro: message(e) }) });
    }
  }
  useEffect(() => { void load(); }, []);

  // No externo o mesmo pedido só completa o config: nada é baixado.
  const install = () => run(async () => {
    haptic.selection();
    setStage({ k: 'installing', at: 0 });
    posting.current = true;
    try { await installCliproxyForServer(server); }
    catch (e) {
      if (failure(e).code === 'erro_cliproxy_ocupado' || !alive.current) return;
      haptic.error();
      setStage({ k: 'install_failed', at: 0, error: message(e) });
    } finally { posting.current = false; }
  });

  // Passa o proxy de fora ao Hangar; com sessões nele pergunta antes, como a atualização.
  const adopt = (confirm = false) => run(async () => {
    haptic.selection();
    setStage({ k: 'installing', at: 0, adopt: true });
    posting.current = true;
    try { await adoptCliproxyForServer(server, confirm); }
    catch (e) {
      const code = failure(e).code;
      if (code === 'erro_cliproxy_ocupado' || !alive.current) return;
      if (code === 'erro_cliproxy_sessoes_ativas') {
        setStage({ k: 'intro', external: true });
        confirmRestart('adopt', Number(failure(e).envelope?.params?.sessions) || 0, () => void adopt(true));
        return;
      }
      haptic.error();
      setStage({ k: 'intro', external: true, error: message(e) });
    } finally { posting.current = false; }
  });

  const installing = stage.k === 'installing';
  useEffect(() => {
    if (!installing) return;
    let stop = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const tick = async () => {
      try {
        // Lido antes da leitura: a resposta que sai com o pedido em voo pode chegar depois de ele terminar.
        const inFlight = posting.current;
        const next = await refresh();
        if (stop) return;
        const job = next.install?.job;
        // Sem tarefa depois do pedido aceito: o servidor reiniciou no meio e o andamento, que era da memória dele, se
        // perdeu. Com o pedido ainda em voo, a tarefa só não nasceu.
        if (!job && !inFlight && !posting.current) { haptic.error(); setStage({ k: 'install_failed', at: 0, error: m.mobile_cliproxy_wizard_install_lost() }); return; }
        if (!job) { if (!stop) timer = setTimeout(() => void tick(), 1000); return; }
        // Com o pedido ainda em voo, uma tarefa concluída é a da tentativa anterior: só a seguinte é desta.
        if (job.done && !inFlight && !posting.current) {
          if (job.error && job.kind === 'adopt') {
            haptic.error();
            // Sessão que passou a usar o proxy durante o download: não é defeito, é só tentar de novo.
            const error = job.error.code === 'erro_cliproxy_sessoes_ativas'
              ? m.mobile_cliproxy_adopt_interrupted() : failureText('adopt', job.error, next.install ?? null);
            setStage({ k: 'intro', external: true, error });
          } else if (job.error) { haptic.error(); setStage({ k: 'install_failed', at: STEP_AT[job.step] ?? 0, error: installError(job.error) }); }
          else { haptic.success(); setStage({ k: 'pick' }); }
          return;
        }
        if (!job.done) {
          const at = STEP_AT[job.step] ?? 0;
          // Tocar com outra tarefa em curso segue a que roda: as etapas mostradas são as dela.
          const adopting = job.kind === 'adopt';
          setStage(s => s.k === 'installing' && (s.at !== at || !!s.adopt !== adopting) ? { ...s, at, adopt: adopting } : s);
        }
      } catch { /* queda de um instante: a próxima leitura decide */ }
      if (!stop) timer = setTimeout(() => void tick(), 1000);
    };
    timer = setTimeout(() => void tick(), 1000);
    return () => { stop = true; clearTimeout(timer); };
  }, [installing]);

  async function openPage(url: string) {
    try {
      const parsed = new URL(url);
      if (parsed.protocol !== 'https:' || !parsed.hostname) throw new Error(url);
      await Linking.openURL(url);
    } catch (e) {
      if (alive.current) setLoginNote(m.mobile_cliproxy_wizard_open_failed({ erro: message(e) }));
    }
  }
  async function beginLogin(provider: Provider) {
    setLoginNote(''); setPaste('');
    try {
      const { url, state } = await startCliproxyLoginForServer(server, provider);
      if (!alive.current) return;
      before.current = new Set((infoNow.current?.connected ?? []).map(a => a.email));
      setStage({ k: 'login', provider, url, state });
      void openPage(url);
    } catch (e) {
      if (!alive.current) return;
      const code = failure(e).code;
      if (code === 'erro_cliproxy_sem_senha') setStage({ k: 'key', provider });
      else if (code === 'erro_cliproxy_senha_recusada') setStage({ k: 'key', provider, error: m.mobile_cliproxy_wizard_err_key_rejected() });
      else { haptic.error(); setStage({ k: 'pick', error: loginError(e) }); }
    }
  }
  const pick = (provider: Provider) => run(async () => { haptic.selection(); await beginLogin(provider); });
  const saveKey = (provider: Provider) => run(async () => {
    const value = key.trim();
    if (!value) return;
    try {
      await putCliproxyManagementKeyForServer(server, value);
      if (!alive.current) return;
      setKey('');
      await beginLogin(provider);
    } catch (e) {
      if (alive.current) { haptic.error(); setStage({ k: 'key', provider, error: message(e) }); }
    }
  });

  const resetKey = (provider: Provider) => run(async () => {
    haptic.selection();
    try {
      await resetCliproxyManagementKeyForServer(server);
      if (!alive.current) return;
      setKey('');
      await beginLogin(provider);
    } catch (e) {
      if (alive.current) { haptic.error(); setStage({ k: 'key', provider, error: message(e) }); }
    }
  });

  const loginState = stage.k === 'login' ? stage.state : null;
  const loginProvider = stage.k === 'login' ? stage.provider : null;
  useEffect(() => {
    if (!loginState || !loginProvider) return;
    // `paused`: erro do login ou recusa que não passa sozinha; a consulta só volta quando a pessoa cola o endereço
    // (ou o app volta à frente). `stop`: este login acabou ou a tela saiu dele.
    let stop = false, reading = false, paused = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    // O login pode terminar no PC: a consulta segue sozinha, não só depois de colar.
    const poll = async () => {
      if (stop || reading) return;
      clearTimeout(timer);
      reading = true; paused = false;
      try {
        const result = await getCliproxyLoginForServer(server, loginState);
        if (stop) return;
        if (result.status === 'ok') { stop = true; await connected(loginProvider, loginState, result.account); return; }
        if (result.status === 'error') {
          paused = true; haptic.error();
          setLoginNote(m.mobile_cliproxy_wizard_login_failed({ erro: result.error ?? '' }));
          return;
        }
      } catch (e) {
        if (stop) return;
        setLoginNote(loginError(e));
        const status = failure(e).status;
        if (status !== undefined && status >= 400 && status < 500 && status !== 408 && status !== 429) paused = true;
      } finally { reading = false; }
      if (!stop && !paused) timer = setTimeout(() => void poll(), 2000);
    };
    pollNow.current = () => void poll();
    timer = setTimeout(() => void poll(), 2000);
    const listener = AppState.addEventListener('change', s => { if (s === 'active') void poll(); });
    return () => { stop = true; clearTimeout(timer); listener.remove(); pollNow.current = () => {}; };
  }, [loginState, loginProvider]);

  async function connected(provider: Provider, state: string, identified?: CliproxyAccount | null) {
    let next = infoNow.current;
    try { next = await refresh(); } catch { /* a conta entrou; sem a lista nova o e-mail fica em branco */ }
    // Voltar ou outro login durante a releitura: a tela já é outra, e este resultado não vale mais.
    const now = stageNow.current;
    if (!alive.current || now.k !== 'login' || now.state !== state) return;
    haptic.success();
    setLoginNote('');
    const own = (next?.connected ?? []).filter(a => a.type === provider);
    if (provider === 'codex') {
      if (!identified?.binding || identified.type !== provider) {
        setStage({ k: 'pick', error: m.cliproxy_choose_connected_account() });
        return;
      }
      setStage({ k: 'connected', account: { provider, email: identified.email, binding: identified.binding, prefix: identified.prefix, plan: identified.plan } });
      return;
    }
    const fresh = own.find(a => !before.current.has(a.email)) ?? own[own.length - 1];
    setStage({ k: 'connected', account: { provider, email: fresh?.email ?? '' } });
  }
  const sendPaste = (state: string) => run(async () => {
    const value = paste.trim();
    if (!CALLBACK.test(value)) { haptic.error(); setLoginNote(m.mobile_cliproxy_wizard_bad_callback()); return; }
    setLoginNote('');
    try {
      await sendCliproxyLoginCallbackForServer(server, state, value);
      if (alive.current) pollNow.current();
    } catch (e) {
      if (!alive.current) return;
      haptic.error();
      setLoginNote(failure(e).status === 400 ? m.mobile_cliproxy_wizard_bad_callback() : loginError(e));
    }
  });

  function toName(account: Account) {
    haptic.selection();
    const models = modelsFor(infoNow.current, account);
    setName(defaultName(account)); setModel(defaultModel(models, account.provider)); setFast(true); setSaveError('');
    setStage({ k: 'name', account });
  }
  const reloadModels = (account: Account) => run(async () => {
    try {
      const next = await refresh();
      if (alive.current) {
        const available = modelsFor(next, account);
        setModel(current => available.includes(current) ? current : defaultModel(available, account.provider));
      }
    } catch (e) { if (alive.current) setSaveError(message(e)); }
  });
  const save = (account: Account) => run(async () => {
    const label = name.trim();
    const id = engineId(label);
    const baseUrl = infoNow.current?.base_url;
    const wantsFast = fastFor(model) && fast;
    const fail = (text: string) => { haptic.error(); setSaveError(text); };
    if (!label) return fail(m.mobile_cliproxy_wizard_name_empty());
    if (!ENGINE_ID.test(id)) return fail(m.mobile_cliproxy_wizard_name_bad());
    if (names.includes(id)) return fail(m.mobile_cliproxy_wizard_name_taken());
    if (!baseUrl) return fail(m.mobile_cliproxy_wizard_load_failed({ erro: infoNow.current?.error ?? m.erro_desconhecido() }));
    if (account.provider === 'codex' && !account.binding) return fail(m.cliproxy_choose_connected_account());
    if (!model || (account.provider === 'codex' && !modelsFor(infoNow.current, account).includes(model))) {
      return fail(m.mobile_cliproxy_wizard_models_empty());
    }
    setSaveError('');
    // Sem janela própria o Claude Code assume 1M; com o limite real menos 5% (margem do Codex), compacta a tempo.
    const prefix = accountPrefix(infoNow.current, account);
    const providerModel = prefix ? `${prefix}/${model}` : model;
    const limit = infoNow.current?.models.find(x => x.id === providerModel)?.context_length;
    try {
      await putEngineForServer(server, id, { label, base_url: baseUrl, model, subagent_model: model,
        use_cliproxy_key: true, create_only: true, experimental_betas: true, fast: wantsFast,
        ...(account.binding ? { cliproxy_binding: account.binding } : {}),
        ...(limit ? { context_window: Math.floor(limit / 100) * 95 } : {}) });
      if (!alive.current) return;
      haptic.success();
      setStage({ k: 'done', account, id, name: label, model, fast: wantsFast });
    } catch (e) {
      // Outra tela criou o mesmo nome depois da lista lida aqui: o servidor recusa em vez de sobrescrever.
      if (alive.current) fail(failure(e).code === 'erro_motor_ja_existe' ? m.mobile_cliproxy_wizard_name_taken() : message(e));
      return;
    }
    // Fora do `try`: o motor já foi gravado; falha de quem chamou não pode virar erro de gravação.
    onSaved();
  });

  const step = stepOf(stage);
  const setup = info?.install;
  const canAdopt = setup?.handover?.ok === true;
  const adoptChosen = adoptPick && canAdopt;
  const existing: Account[] =(info?.connected ?? []).flatMap(a =>
    a.type === 'codex' || a.type === 'claude' ? [{ provider: a.type, email: a.email, binding: a.binding, prefix: a.prefix, plan: a.plan }] : []);
  const body = (() => {
    switch (stage.k) {
      case 'loading': return <View style={styles.center}><ActivityIndicator color={c.muted} />
        <Text style={[styles.muted, { color: c.muted }]}>{m.comum_carregando()}</Text></View>;
      case 'load_error': return <>
        <ErrorBox text={stage.error} />
        <Foot><Button primary label={m.mobile_cliproxy_wizard_retry()} onPress={() => void load()} /></Foot>
      </>;
      case 'intro': return stage.external ? <>
        <Text style={[styles.lead, { color: c.text }]}>{setup?.version
          ? m.mobile_cliproxy_wizard_external_version({ version: setup.version }) : m.mobile_cliproxy_wizard_external_lead()}</Text>
        <Choice selected={adoptChosen} disabled={busy || !canAdopt} recommended title={m.mobile_cliproxy_wizard_adopt_title()}
          desc={setup?.handover && !setup.handover.ok ? handoverText(setup.handover.code) : m.mobile_cliproxy_wizard_adopt_desc()}
          onPress={() => setAdoptPick(true)} />
        <Choice selected={!adoptChosen} disabled={busy} title={m.mobile_cliproxy_wizard_keep_title()} desc={m.mobile_cliproxy_wizard_keep_desc()}
          onPress={() => setAdoptPick(false)} />
        {stage.error ? <ErrorBox text={stage.error} /> : null}
        <Foot>
          <Button label={m.comum_cancelar()} disabled={busy} onPress={onClose} />
          <Button primary label={stage.error ? m.mobile_cliproxy_wizard_retry() : m.mobile_cliproxy_wizard_continue()} disabled={busy}
            onPress={() => void (adoptChosen ? adopt() : install())} />
        </Foot>
      </> : <>
        <Text style={[styles.lead, { color: c.text }]}>{m.mobile_cliproxy_wizard_install_lead()}</Text>
        <Text style={[styles.muted, { color: c.muted }]}>{m.mobile_cliproxy_wizard_install_desc()}</Text>
        <InstallItems at={-1} failed={false} />
        <Foot>
          <Button label={m.comum_cancelar()} disabled={busy} onPress={onClose} />
          <Button primary label={m.mobile_cliproxy_wizard_install()} disabled={busy} onPress={() => void install()} />
        </Foot>
      </>;
      case 'installing': return <>
        <Text style={[styles.lead, { color: c.text }]} accessibilityLiveRegion="polite">
          {stage.adopt ? m.mobile_cliproxy_adopt_running() : m.mobile_cliproxy_wizard_installing()}</Text>
        <InstallItems at={stage.at} failed={false} labels={stage.adopt ? [m.mobile_cliproxy_adopt_step_download({ new: setup?.latest ?? '' }),
          m.mobile_cliproxy_adopt_step_config(), m.mobile_cliproxy_adopt_step_service(), m.mobile_cliproxy_adopt_step_start()] : undefined} />
        <Foot><Button primary label={m.mobile_cliproxy_wizard_installing()} disabled onPress={() => {}} /></Foot>
      </>;
      case 'install_failed': return <>
        <Text style={[styles.lead, { color: c.text }]}>{m.mobile_cliproxy_wizard_install_failed()}</Text>
        <InstallItems at={stage.at} failed />
        <ErrorBox text={stage.error} />
        <Foot><Button primary label={m.mobile_cliproxy_wizard_retry()} disabled={busy} onPress={() => void install()} /></Foot>
      </>;
      case 'pick': return <>
        <Text style={[styles.lead, { color: c.text }]}>{m.mobile_cliproxy_wizard_pick_lead()}</Text>
        <Text style={[styles.muted, { color: c.muted }]}>{m.mobile_cliproxy_wizard_pick_desc()}</Text>
        {existing.length ? <>
          <Text style={[styles.small, styles.strong, { color: c.muted }]}>{m.mobile_cliproxy_wizard_existing()}</Text>
          {existing.map(account => <AccountLine key={account.binding || `${account.provider}:${account.email}`} account={account}
            disabled={busy || (account.provider === 'codex' && !account.binding)} onUse={() => toName(account)} />)}
          <Text style={[styles.small, styles.strong, { color: c.muted }]}>{m.mobile_cliproxy_wizard_new_account()}</Text>
        </> : null}
        <ProviderCard provider="codex" title={m.mobile_cliproxy_wizard_chatgpt()} desc={m.mobile_cliproxy_wizard_chatgpt_desc()}
          recommended disabled={busy} onPress={() => void pick('codex')} />
        <ProviderCard provider="claude" title={m.mobile_cliproxy_wizard_claude()} desc={m.mobile_cliproxy_wizard_claude_desc()}
          disabled={busy} onPress={() => void pick('claude')} />
        {busy ? <Text style={[styles.muted, { color: c.muted }]} accessibilityLiveRegion="polite">{m.mobile_cliproxy_wizard_starting_login()}</Text> : null}
        {stage.error ? <ErrorBox text={stage.error} /> : null}
      </>;
      case 'key': return <>
        <Text style={[styles.lead, { color: c.text }]}>{m.mobile_cliproxy_wizard_key_needed()}</Text>
        <TextInput accessibilityLabel={m.cliproxy_senha_placeholder()} placeholder={m.cliproxy_senha_placeholder()} value={key}
          onChangeText={setKey} secureTextEntry autoCapitalize="none" autoCorrect={false} editable={!busy}
          placeholderTextColor={c.faint} style={[styles.input, { color: c.text, borderColor: c.borderStrong, backgroundColor: c.inset }]} />
        {stage.error ? <ErrorBox text={stage.error} /> : null}
        <Text style={[styles.muted, { color: c.muted }]}>{m.mobile_cliproxy_wizard_key_reset_help()}</Text>
        <Button label={m.mobile_cliproxy_wizard_key_reset()} disabled={busy} onPress={() => void resetKey(stage.provider)} />
        <Foot>
          <Button link label={m.mobile_cliproxy_wizard_back()} disabled={busy} onPress={() => setStage({ k: 'pick' })} />
          <Button primary label={m.mobile_cliproxy_wizard_key_save()} disabled={busy || !key.trim()} onPress={() => void saveKey(stage.provider)} />
        </Foot>
      </>;
      case 'login': return <>
        <View style={[styles.box, { borderColor: c.borderStrong }]}>
          <View style={styles.boxHead}>
            <ActivityIndicator color={c.accent} />
            <View style={styles.flex}>
              <Text style={[styles.strong, { color: c.text }]}>{m.mobile_cliproxy_wizard_login_title({ provider: providerName(stage.provider) })}</Text>
              <Text style={[styles.muted, { color: c.muted }]}>{m.mobile_cliproxy_wizard_login_phone()}</Text>
            </View>
          </View>
          <TextInput accessibilityLabel={m.mobile_cliproxy_wizard_paste_label()} placeholder={m.mobile_cliproxy_wizard_paste_placeholder()}
            value={paste} onChangeText={setPaste} autoCapitalize="none" autoCorrect={false} keyboardType="url" editable={!busy}
            placeholderTextColor={c.faint} style={[styles.input, styles.mono, { color: c.text, borderColor: c.borderStrong, backgroundColor: c.inset }]} />
          <Button primary label={m.mobile_cliproxy_wizard_finish_login()} disabled={busy || !paste.trim()} onPress={() => void sendPaste(stage.state)} />
          {loginNote ? <ErrorBox text={loginNote} /> : null}
        </View>
        <Foot>
          <Button link label={m.mobile_cliproxy_wizard_back()} disabled={busy} onPress={() => { setLoginNote(''); setStage({ k: 'pick' }); }} />
          <Button label={m.mobile_cliproxy_wizard_reopen()} disabled={busy} onPress={() => { haptic.selection(); void openPage(stage.url); }} />
        </Foot>
      </>;
      case 'connected': return <>
        <Text style={[styles.lead, { color: c.text }]}>{m.mobile_cliproxy_wizard_connected_lead()}</Text>
        <AccountLine account={stage.account} />
        <Foot>
          <Button link label={m.mobile_cliproxy_wizard_another()} onPress={() => { haptic.selection(); setStage({ k: 'pick' }); }} />
          <Button primary label={m.mobile_cliproxy_wizard_continue()} onPress={() => toName(stage.account)} />
        </Foot>
      </>;
      case 'name': {
        const models = modelsFor(info, stage.account);
        return <>
          <AccountLine account={stage.account} />
          <View style={styles.field}>
            <Text style={[styles.label, { color: c.text }]}>{m.mobile_cliproxy_wizard_name()}</Text>
            <TextInput accessibilityLabel={m.mobile_cliproxy_wizard_name()} value={name} onChangeText={setName} editable={!busy}
              autoCorrect={false} placeholderTextColor={c.faint}
              style={[styles.input, { color: c.text, borderColor: c.borderStrong, backgroundColor: c.inset }]} />
            <Preview label={m.mobile_cliproxy_wizard_name_preview()} name={name.trim() || '…'} />
          </View>
          <View style={styles.field}>
            <Text style={[styles.label, { color: c.text }]}>{m.mobile_cliproxy_wizard_model()}</Text>
            {models.length ? <View accessibilityRole="radiogroup" accessibilityLabel={m.mobile_cliproxy_wizard_model()}
              style={[styles.group, { borderColor: c.borderStrong }]}>
              {models.map((id, i) => <Pressable key={id} accessibilityRole="radio" accessibilityLabel={id}
                accessibilityState={{ checked: id === model, disabled: busy }} disabled={busy}
                onPress={() => { if (id === model) return; haptic.selection(); setModel(id); }}
                style={({ pressed }) => [styles.option, i > 0 && { borderTopWidth: StyleSheet.hairlineWidth, borderTopColor: c.border },
                  pressed && { backgroundColor: c.hover }]}>
                <Text style={[styles.optionText, { color: c.text }]} numberOfLines={1}>{id}</Text>
                {id === model ? <Icon name="Check" size={18} color={c.accent} /> : null}
              </Pressable>)}
            </View> : <>
              <Text style={[styles.muted, { color: c.muted }]}>{stage.account.provider === 'codex' && !accountPrefix(info, stage.account)
                ? m.cliproxy_account_needs_prefix() : m.mobile_cliproxy_wizard_models_empty()}</Text>
              <Button label={m.mobile_cliproxy_wizard_models_reload()} disabled={busy} onPress={() => void reloadModels(stage.account)} />
            </>}
            <Text style={[styles.muted, { color: c.muted }]}>{m.mobile_cliproxy_wizard_model_desc()}</Text>
          </View>
          {fastFor(model) ? <View style={styles.field}>
            <Text style={[styles.label, { color: c.text }]}>{m.mobile_cliproxy_wizard_speeds()}</Text>
            <FastRow value={fast} disabled={busy} onValueChange={value => { haptic.selection(); setFast(value); }} />
            <Text style={[styles.muted, { color: c.muted }]}>{m.mobile_cliproxy_wizard_speeds_desc()}</Text>
          </View> : null}
          {saveError ? <ErrorBox text={saveError} /> : null}
          <Foot><Button primary label={busy ? m.mobile_cliproxy_wizard_saving() : m.mobile_cliproxy_wizard_finish()}
            disabled={busy} onPress={() => void save(stage.account)} /></Foot>
        </>;
      }
      case 'done': return <>
        <View style={styles.doneTop}>
          <View style={[styles.doneMark, { backgroundColor: `${theme.tokens.status.success}2e` }]}>
            <Icon name="Check" size={18} color={theme.tokens.status.success} />
          </View>
          <View style={styles.flex}>
            <Text accessibilityRole="header" style={[styles.title, { color: c.text }]}>{m.mobile_cliproxy_wizard_done_title()}</Text>
            <Text style={[styles.muted, { color: c.muted }]}>{m.mobile_cliproxy_wizard_done_desc({ name: stage.name })}</Text>
          </View>
        </View>
        <Preview label={m.mobile_cliproxy_wizard_done_preview()} name={stage.name}
          extra={stage.fast ? `${stage.model} · ${m.mobile_cliproxy_wizard_fast_available()}` : stage.model} />
        <Foot>
          <Button label={m.sessao_fechar()} onPress={onClose} />
          <Button primary label={m.mobile_cliproxy_wizard_open_session({ name: stage.name })} onPress={() => {
            haptic.selection();
            onClose();
            router.push({ pathname: '/create', params: { server: server.id, engine: stage.id } } as never);
          }} />
        </Foot>
      </>;
    }
  })();

  return (
    <Sheet open sizes={['large']} scrollable dismissible={!busy} onDismiss={onClose}>
      <KeyboardAvoidingView behavior="padding" automaticOffset style={styles.flex}>
        <ScrollView keyboardShouldPersistTaps="handled" keyboardDismissMode="on-drag"
          contentContainerStyle={[styles.body, { paddingBottom: insets.bottom + theme.base.space[6] }]}>
          <View style={styles.head}>
            <View style={[styles.badge, { backgroundColor: c.accentDim }]}>
              <Text style={[styles.badgeText, { color: c.accentText }]}>{m.mobile_cliproxy_wizard_badge()}</Text>
            </View>
            <View style={styles.flex}>
              <Text accessibilityRole="header" style={[styles.title, { color: c.text }]}>{m.mobile_cliproxy_wizard_title()}</Text>
              <Text style={[styles.small, { color: c.muted }]}>{m.mobile_cliproxy_wizard_subtitle()}</Text>
            </View>
            <Pressable accessibilityRole="button" accessibilityLabel={m.sessao_fechar()} disabled={busy} onPress={onClose} style={styles.close}>
              <Icon name="X" size={20} color={c.muted} />
            </Pressable>
          </View>
          <Stepper current={step} allDone={stage.k === 'done'} />
          {body}
        </ScrollView>
      </KeyboardAvoidingView>
    </Sheet>
  );
}

export function Stepper({ current, allDone }: { current: number; allDone: boolean }) {
  const c = useSettingsColors();
  const { theme } = useUnistyles();
  const labels = [m.mobile_cliproxy_wizard_step_install(), m.mobile_cliproxy_wizard_step_connect(), m.mobile_cliproxy_wizard_step_name()];
  return <View style={styles.steps} accessible accessibilityLabel={m.mobile_cliproxy_wizard_step_a11y({ n: String(current), label: labels[current - 1] })}>
    {labels.map((label, i) => {
      const n = i + 1;
      const done = allDone || n < current;
      const now = !done && n === current;
      return <View key={label} style={[styles.stepWrap, i > 0 && styles.flexStep]}>
        {i > 0 ? <View style={[styles.stepLine, { backgroundColor: n <= current ? c.accent : c.borderStrong }]} /> : null}
        <View style={[styles.stepDot, { borderColor: done || now ? c.accent : c.borderStrong }, done && { backgroundColor: c.accent }]}>
          {done ? <Icon name="Check" size={12} color={theme.tokens.text.inverse} />
            : <Text style={[styles.stepNum, { color: now ? c.accent : c.muted }]}>{n}</Text>}
        </View>
        {now ? <Text style={[styles.stepLabel, { color: c.text }]} numberOfLines={1}>{label}</Text> : null}
      </View>;
    })}
  </View>;
}


function ProviderCard({ provider, title, desc, recommended, disabled, onPress }: {
  provider: Provider; title: string; desc: string; recommended?: boolean; disabled: boolean; onPress: () => void;
}) {
  const c = useSettingsColors();
  return <Pressable accessibilityRole="button" accessibilityLabel={recommended ? `${title}, ${m.mobile_cliproxy_wizard_recommended()}` : title}
    accessibilityHint={desc} accessibilityState={{ disabled }} disabled={disabled} onPress={onPress}
    style={({ pressed }) => [styles.card, { borderColor: pressed ? c.accent : c.borderStrong, opacity: disabled ? 0.5 : 1 },
      pressed && { backgroundColor: c.accentDim }]}>
    <View style={styles.cardTitle}>
      <ProviderGlyph provider={provider} size={26} />
      <Text style={[styles.strong, styles.flex, { color: c.text }]}>{title}</Text>
      {recommended ? <Text style={[styles.rec, { color: c.accentText, backgroundColor: c.accentDim }]}>{m.mobile_cliproxy_wizard_recommended()}</Text> : null}
    </View>
    <Text style={[styles.small, { color: c.muted }]}>{desc}</Text>
  </Pressable>;
}

/** Uma das duas escolhas do proxy de fora: marcar só escolhe, quem age é o Continuar. */
function Choice({ selected, disabled, recommended, title, desc, onPress }: {
  selected: boolean; disabled: boolean; recommended?: boolean; title: string; desc: string; onPress: () => void;
}) {
  const c = useSettingsColors();
  return <Pressable accessibilityRole="radio" accessibilityLabel={recommended ? `${title}, ${m.mobile_cliproxy_wizard_recommended()}` : title}
    accessibilityHint={desc} accessibilityState={{ checked: selected, disabled }} disabled={disabled}
    onPress={() => { haptic.selection(); onPress(); }}
    style={[styles.card, { borderColor: selected ? c.accent : c.borderStrong, opacity: disabled && !selected ? 0.6 : 1 },
      selected && { backgroundColor: c.accentDim }]}>
    <View style={styles.cardTitle}>
      <View style={[styles.radio, { borderColor: selected ? c.accent : c.borderStrong }]}>
        {selected ? <View style={[styles.radioDot, { backgroundColor: c.accent }]} /> : null}
      </View>
      <Text style={[styles.strong, styles.flex, { color: c.text }]}>{title}</Text>
      {recommended ? <Text style={[styles.rec, { color: c.accentText, backgroundColor: c.accentDim }]}>{m.mobile_cliproxy_wizard_recommended()}</Text> : null}
    </View>
    <Text style={[styles.small, { color: c.muted }]}>{desc}</Text>
  </Pressable>;
}

// Com `onUse` é a conta que já estava no proxy: o botão segue para o nome sem abrir login.
export function AccountLine({ account, onUse, disabled }: { account: Account; onUse?: () => void; disabled?: boolean }) {
  const c = useSettingsColors();
  const { theme } = useUnistyles();
  return <View style={[styles.account, { borderColor: c.borderStrong }]}>
    <ProviderGlyph provider={account.provider} size={26} />
    <Text style={[styles.optionText, { color: c.text }]} numberOfLines={2}>
      {[account.plan, account.prefix, account.email || providerName(account.provider)].filter(Boolean).join(' · ')}
    </Text>
    {onUse ? <Button label={m.mobile_cliproxy_wizard_use_account()} disabled={disabled} onPress={onUse} /> : <View style={styles.ok}>
      <Icon name="Check" size={14} color={theme.tokens.status.success} />
      <Text style={[styles.small, { color: theme.tokens.status.success }]}>{m.mobile_cliproxy_wizard_connected()}</Text>
    </View>}
  </View>;
}

export function Preview({ label, name, extra }: { label: string; name: string; extra?: string }) {
  const c = useSettingsColors();
  return <View style={[styles.preview, { borderColor: c.borderStrong }]}>
    <Text style={[styles.small, { color: c.muted }]}>{label}</Text>
    <Text style={[styles.pill, { color: c.text, backgroundColor: c.hover }]} numberOfLines={1}>{name}</Text>
    {extra ? <Text style={[styles.small, { color: c.muted }]} numberOfLines={1}>{extra}</Text> : null}
  </View>;
}

export function FastRow({ value, disabled, onValueChange }: { value: boolean; disabled?: boolean; onValueChange?: (value: boolean) => void }) {
  const c = useSettingsColors();
  const { theme } = useUnistyles();
  return <View style={[styles.tier, { borderColor: c.borderStrong }]}>
    <View style={styles.flex}>
      <Text style={[styles.strong, { color: c.text }]}>{m.mobile_cliproxy_wizard_fast()}</Text>
      <Text style={[styles.small, { color: c.muted }]}>{m.mobile_cliproxy_wizard_fast_desc()}</Text>
    </View>
    <Switch accessibilityLabel={m.mobile_cliproxy_wizard_fast()} value={value} disabled={disabled}
      trackColor={{ true: theme.tokens.status.success }} onValueChange={onValueChange} />
  </View>;
}

function ErrorBox({ text }: { text: string }) {
  const { theme } = useUnistyles();
  return <Text accessibilityRole="alert" style={[styles.error, { color: theme.tokens.status.error,
    backgroundColor: `${theme.tokens.status.error}1f` }]}>{text}</Text>;
}

function Foot({ children }: { children: ReactNode }) {
  return <View style={styles.foot}>{children}</View>;
}

function Button({ label, onPress, disabled, primary, link }: {
  label: string; onPress: () => void; disabled?: boolean; primary?: boolean; link?: boolean;
}) {
  const c = useSettingsColors();
  const { theme } = useUnistyles();
  return <Pressable accessibilityRole="button" accessibilityLabel={label} accessibilityState={{ disabled: !!disabled }}
    disabled={disabled} onPress={onPress}
    style={({ pressed }) => [styles.button, primary && styles.flex,
      primary ? { backgroundColor: c.text } : link ? null : { borderWidth: 1, borderColor: c.borderStrong },
      { opacity: disabled ? 0.45 : pressed && primary ? 0.8 : 1 }, pressed && !primary && { backgroundColor: c.hover }]}>
    <Text style={[styles.buttonText, { color: primary ? theme.tokens.text.inverse : link ? c.muted : c.text }]} numberOfLines={1}>{label}</Text>
  </Pressable>;
}

const styles = StyleSheet.create(theme => ({
  flex: { flex: 1 },
  body: { padding: theme.base.space[4], gap: theme.base.space[3] },
  head: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[3] },
  badge: { width: 40, height: 40, borderRadius: theme.base.radius.md, borderCurve: 'continuous', alignItems: 'center', justifyContent: 'center' },
  badgeText: { fontSize: theme.base.text.sm, fontWeight: theme.base.weight.semibold },
  title: { fontSize: theme.base.text.base, fontWeight: theme.base.weight.semibold },
  close: { width: 44, height: 44, alignItems: 'center', justifyContent: 'center', alignSelf: 'flex-start', marginTop: -theme.base.space[1] },
  steps: { flexDirection: 'row', alignItems: 'center', marginVertical: theme.base.space[2] },
  stepWrap: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2] },
  flexStep: { flex: 1 },
  stepLine: { flex: 1, height: 1.5, marginHorizontal: theme.base.space[1], minWidth: 14 },
  stepDot: { width: 24, height: 24, borderRadius: theme.base.radius.full, borderWidth: 1.5, alignItems: 'center', justifyContent: 'center' },
  stepNum: { fontSize: theme.base.text.xxs, fontWeight: theme.base.weight.semibold },
  stepLabel: { fontSize: theme.base.text.xs, fontWeight: theme.base.weight.medium, flexShrink: 1 },
  lead: { fontSize: theme.base.text.sm, lineHeight: 21 },
  muted: { fontSize: theme.base.text.xs, lineHeight: 18 },
  small: { fontSize: theme.base.text.xs },
  strong: { fontSize: theme.base.text.sm, fontWeight: theme.base.weight.semibold },
  center: { alignItems: 'center', gap: theme.base.space[2], paddingVertical: theme.base.space[8] },
  foot: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2], marginTop: theme.base.space[2] },
  button: { minHeight: 44, paddingHorizontal: theme.base.space[4], borderRadius: theme.base.radius.md, borderCurve: 'continuous',
    alignItems: 'center', justifyContent: 'center' },
  buttonText: { fontSize: theme.base.text.sm, fontWeight: theme.base.weight.semibold },
  card: { minHeight: 44, borderWidth: 1, borderRadius: theme.base.radius.md, borderCurve: 'continuous', padding: theme.base.space[3], gap: theme.base.space[1] },
  cardTitle: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2] },
  radio: { width: 18, height: 18, borderRadius: theme.base.radius.full, borderWidth: 1.5, alignItems: 'center', justifyContent: 'center' },
  radioDot: { width: 8, height: 8, borderRadius: theme.base.radius.full },
  rec: { fontSize: theme.base.text.xxs, fontWeight: theme.base.weight.medium, paddingHorizontal: theme.base.space[2], paddingVertical: 2,
    borderRadius: theme.base.radius.full, overflow: 'hidden' },
  box: { borderWidth: 1, borderRadius: theme.base.radius.md, borderCurve: 'continuous', padding: theme.base.space[3], gap: theme.base.space[3] },
  boxHead: { flexDirection: 'row', alignItems: 'flex-start', gap: theme.base.space[3] },
  input: { minHeight: 44, borderWidth: 1, borderRadius: theme.base.radius.sm, paddingHorizontal: theme.base.space[3],
    paddingVertical: theme.base.space[2], fontSize: theme.base.text.sm },
  mono: { fontFamily: theme.base.fontMono, fontSize: theme.base.text.xs },
  error: { fontSize: theme.base.text.xs, lineHeight: 18, padding: theme.base.space[3], borderRadius: theme.base.radius.md, overflow: 'hidden' },
  account: { minHeight: 44, flexDirection: 'row', alignItems: 'center', gap: theme.base.space[3], borderWidth: 1,
    borderRadius: theme.base.radius.md, borderCurve: 'continuous', paddingHorizontal: theme.base.space[3], paddingVertical: theme.base.space[2] },
  ok: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[1] },
  field: { gap: theme.base.space[2] },
  label: { fontSize: theme.base.text.xs, fontWeight: theme.base.weight.medium },
  group: { borderWidth: 1, borderRadius: theme.base.radius.md, borderCurve: 'continuous', overflow: 'hidden' },
  option: { minHeight: 44, flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2], paddingHorizontal: theme.base.space[3] },
  optionText: { flex: 1, fontSize: theme.base.text.sm },
  tier: { minHeight: 44, flexDirection: 'row', alignItems: 'center', gap: theme.base.space[3], borderWidth: 1,
    borderRadius: theme.base.radius.md, borderCurve: 'continuous', paddingHorizontal: theme.base.space[3], paddingVertical: theme.base.space[2] },
  preview: { flexDirection: 'row', flexWrap: 'wrap', alignItems: 'center', gap: theme.base.space[2], borderWidth: 1, borderStyle: 'dashed',
    borderRadius: theme.base.radius.md, paddingHorizontal: theme.base.space[3], paddingVertical: theme.base.space[2] },
  pill: { fontSize: theme.base.text.xs, fontWeight: theme.base.weight.medium, paddingHorizontal: theme.base.space[2], paddingVertical: 3,
    borderRadius: theme.base.radius.sm, overflow: 'hidden', maxWidth: '100%' },
  doneTop: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[3] },
  doneMark: { width: 36, height: 36, borderRadius: theme.base.radius.full, alignItems: 'center', justifyContent: 'center' },
}));
