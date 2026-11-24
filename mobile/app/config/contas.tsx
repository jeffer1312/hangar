import { lazy, Suspense, useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { Alert, ScrollView, Text, View } from 'react-native';
import { StyleSheet } from 'react-native-unistyles';
import { ACCOUNT_FILTERS, accountInFilter, accountMatches, codexCliAusente, credentialAuth, credentialGroup, deleteCodexAccountForServer,
  getCodexAccountsForServer, getCredentialsForServer, accountDeletedNotice, motivoLoginIndisponivel, parseAccountSort, sortAccounts,
  getAntigravityAccountsForServer, deleteAntigravityAccountForServer, signOutAntigravityAccountForServer, providerName,
  type AccountFilter, type AccountSort, type AntigravityAccount, type AntigravityAccounts, type CodexAccount, type Credencial, type Server } from '@hangar/core';
import { Pagina } from '../../src/features/config/Pagina';
import { PageHeader, Pill, type PageAction } from '../../src/features/config/PageHeader';
import { SectionCard } from '../../src/features/config/SectionCard';
import { InfoNotice } from '../../src/features/config/InfoNotice';
import { CodexContaLogin } from '../../src/features/config/CodexContaLogin';
import { AntigravityContaLogin } from '../../src/features/config/AntigravityContaLogin';
import { antigravityAccountLabel, antigravityLoginText, isAdditionalAntigravity } from '../../src/features/accounts/antigravity';
import { ResetFeedback } from '../../src/features/accounts/CodexReset';
import { EngineStatus, useEngines } from '../../src/features/accounts/Engines';
import { ProxyCard, managedVersion, useProxyUpdate } from '../../src/features/accounts/ProxyUpdate';
import { ClaudeAccountCreate } from '../../src/features/accounts/ClaudeAccountCreate';
import { AccountRow, type AccountRowData, type AccountRowHandlers } from '../../src/features/accounts/AccountRow';
import { AccountSearch, FilterChips, SortControl, type SortOption } from '../../src/features/accounts/AccountsToolbar';
import { useServers } from '../../src/stores/servers';
import { prefs } from '../../src/stores/prefs';
import * as m from '../../src/paraglide/messages';

type CredentialGroup = ReturnType<typeof credentialGroup>;
type Subject = Pick<Credencial, 'tipo' | 'nome' | 'nome_natural' | 'login' | 'cota' | 'ativa'>;
type IdentityRow = AccountRowData & { group: CredentialGroup; subject: Subject };
const EngineEditor = lazy(() => import('../../src/features/accounts/EngineEditor').then(module => ({ default: module.EngineEditor })));
const CliproxyWizard = lazy(() => import('../../src/features/accounts/CliproxyWizard').then(module => ({ default: module.CliproxyWizard })));
const capitalize = (plan: string | null | undefined) => plan ? `${plan.charAt(0).toUpperCase()}${plan.slice(1)}` : '';
const SORT_KEY = 'accounts_sort';
// A ordem dos grupos de antes vira a ordem recebida da lista única.
const GROUP_ORDER: CredentialGroup[] = ['subscription', 'claude_engine', 'api_key', 'unknown'];
const windowLabel = (label: string) => label === '5h' ? m.rate_5h() : label === '7d' ? m.accounts_window_week() : label;
const windowRank = (label: string) => label === '5h' ? 0 : label === '7d' ? 1 : 2;
const readSort = (): AccountSort => {
  try { return parseAccountSort(prefs.getString(SORT_KEY)); } catch { return null; }
};
const providerOf = (credential: Credencial | null, account: CodexAccount | null): AccountRowData['provider'] => {
  if (credential?.tipo === 'claude') return 'claude';
  if (credential?.tipo === 'codex' || account) return 'codex';
  if (credential?.tipo === 'antigravity') return 'antigravity';
  if (credential && (credential.id.startsWith('kimi:') || credential.base_url?.includes('kimi'))) return 'kimi';
  return '';
};

export default function Contas() {
  const server = useServers((state) => state.active());
  return <ServerAccounts key={JSON.stringify([server?.id, server?.baseUrl, server?.token])} server={server} />;
}

function ServerAccounts({ server }: { server: Server | null }) {
  const [accounts, setAccounts] = useState<CodexAccount[]>([]);
  const [credentials, setCredentials] = useState<Credencial[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState('');
  const [loginAccount, setLoginAccount] = useState<string | null>(null);
  const [newLogin, setNewLogin] = useState(false);
  const [newClaude, setNewClaude] = useState(false);
  const [cliproxyWizard, setCliproxyWizard] = useState(false);
  // Contas Google do Antigravity: lista própria, mais lenta (o servidor roda o `agy models` de cada uma).
  const [agy, setAgy] = useState<AntigravityAccounts | null>(null);
  const [agyError, setAgyError] = useState('');
  const [agyLogin, setAgyLogin] = useState<{ accountId?: string } | null>(null);
  const agyGeneration = useRef(0);
  const [deleting, setDeleting] = useState<ReadonlySet<string>>(new Set());
  // Trava síncrona: dois alertas abertos antes do primeiro sim ainda veem o `deleting` antigo.
  const deletingNow = useRef(new Set<string>());
  const generation = useRef(0);
  const controller = useRef<AbortController | null>(null);
  const alive = useRef(true);
  const [filter, setFilter] = useState<AccountFilter>('all');
  const [term, setTerm] = useState('');
  const [sort, setSortState] = useState<AccountSort>(readSort);
  const setSort = useCallback((next: AccountSort) => {
    setSortState(next);
    try { if (next) prefs.set(SORT_KEY, JSON.stringify(next)); else prefs.remove(SORT_KEY); } catch { /* preferência local: falhar só não lembra a ordem */ }
  }, []);
  const rowCache = useRef(new Map<string, { sig: string; row: IdentityRow }>());
  const painted = useRef(false);
  const handlersNow = useRef<AccountRowHandlers | null>(null);
  // Mesmo objeto em toda pintura: as linhas memorizadas não redesenham por causa dele.
  const handlers = useMemo<AccountRowHandlers>(() => ({
    signIn: (id) => handlersNow.current?.signIn(id),
    remove: (account) => handlersNow.current?.remove(account),
    editEngine: (name) => handlersNow.current?.editEngine(name),
    removeEngine: (name) => handlersNow.current?.removeEngine(name),
    refreshClaude: () => handlersNow.current?.refreshClaude() ?? Promise.resolve(),
    antigravitySignIn: (id) => handlersNow.current?.antigravitySignIn(id),
    antigravitySignOut: (account) => handlersNow.current?.antigravitySignOut(account),
    antigravityRemove: (account) => handlersNow.current?.antigravityRemove(account),
  }), []);

  const agyTarget = useRef('');
  const loadAgy = useCallback(async (target: Server | null) => {
    const generationNow = ++agyGeneration.current;
    if (!target) { setAgy(null); setAgyError(''); agyTarget.current = ''; return; }
    // Outra máquina: a lista da anterior não pode ficar à mostra com ações apontando para ela.
    const key = `${target.id}|${target.baseUrl}`;
    if (agyTarget.current !== key) { setAgy(null); agyTarget.current = key; }
    try {
      const next = await getAntigravityAccountsForServer(target);
      if (!alive.current || generationNow !== agyGeneration.current) return;
      setAgy(next);
      setAgyError('');
    } catch (cause: unknown) {
      if (!alive.current || generationNow !== agyGeneration.current) return;
      // Servidor sem a rota é de antes do Antigravity: a lista segue sem ele.
      if ((cause as { status?: number } | null)?.status === 404) { setAgy(null); setAgyError(''); return; }
      setAgyError(cause instanceof Error ? cause.message : String(cause));
    }
  }, []);

  const load = useCallback(async (target: typeof server, force = false, propagate = false) => {
    if (!alive.current) return;
    const generationNow = ++generation.current;
    controller.current?.abort();
    const nextController = new AbortController();
    controller.current = nextController;
    setError('');
    void loadAgy(target);
    if (!target) {
      setLoading(false);
      return;
    }
    setLoading(true);
    try {
      const [next, nextCredentials] = await Promise.all([
        getCodexAccountsForServer(target, nextController.signal),
        getCredentialsForServer(target, force, nextController.signal),
      ]);
      if (generationNow !== generation.current || nextController.signal.aborted) return;
      setAccounts(next);
      setCredentials(nextCredentials);
    } catch (cause: unknown) {
      if (generationNow !== generation.current || nextController.signal.aborted) return;
      const message = cause instanceof Error ? cause.message : m.codex_ui_login_error();
      setError(message);
      if (propagate) throw new Error(message);
    } finally {
      if (generationNow === generation.current && !nextController.signal.aborted) setLoading(false);
    }
  }, [loadAgy]);

  const engines = useEngines(server, () => load(server));
  const proxy = useProxyUpdate(server);
  const proxyVersion = managedVersion(proxy.install);
  // A versão do proxy do Hangar vai na linha do modelo que ele serve.
  const proxyNote = (engine: string | undefined) =>
    proxyVersion && engine && engines.data?.motores[engine]?.cliproxy_accounts ? m.mobile_accounts_proxy_version({ version: proxyVersion }) : '';
  const refreshQuota = useCallback(() => load(server, true), [load, server]);
  const refreshClaude = useCallback(() => load(server, true, true), [load, server]);
  useEffect(() => {
    alive.current = true;
    setLoginAccount(null);
    setNewLogin(false);
    setNewClaude(false);
    setAgyLogin(null);
    void load(server);
    return () => {
      alive.current = false;
      controller.current?.abort();
      generation.current++;
    };
  }, [load, server?.id, server?.baseUrl, server?.token]);

  const selected = loginAccount === null ? null : accounts.find((account) => account.id === loginAccount) ?? null;
  const accountForCredential = (credential: Credencial) => {
    if (credential.tipo !== 'codex') return null;
    return accounts.find((account) => account.id === credential.codex_account || account.credential_id === credential.id) ?? null;
  };
  // A conta Codex não traz a causa da falha; a linha da credencial dela traz.
  // Email e plano têm lugar próprio na linha; a descrição fica com o resto.
  const accountStatus = (account: CodexAccount, credential?: Credencial) => {
    const auth = account.auth;
    if (auth.status === 'connected') return { email: auth.email ?? '', plan: capitalize(auth.plan), description: auth.email ? '' : m.codex_ui_account() };
    const description = auth.status === 'disconnected' ? m.contas_nao_conectada()
      : codexCliAusente(account) ? m.codex_ui_cli_ausente()
      : (credential && motivoLoginIndisponivel(credential)) || m.codex_ui_unknown();
    return { email: '', plan: '', description };
  };
  const groupForAccount = (account: CodexAccount): CredentialGroup => (
    account.auth.method === 'oauth' ? 'subscription' : account.auth.method === 'api_key' ? 'api_key' : 'unknown'
  );
  const canSignIn = (account: CodexAccount) => account.auth.status !== 'connected' && !codexCliAusente(account);
  const credentialStatus = (credential: Credencial, title: string) => {
    const auth = credentialAuth(credential);
    const naoConferido = motivoLoginIndisponivel(credential);
    const only = (description: string) => ({ email: '', plan: '', description });
    if (auth === 'unknown') return only(codexCliAusente(credential) ? m.codex_ui_cli_ausente() : naoConferido ?? m.codex_ui_unknown());
    if (auth === 'none' || credential.login?.loggedIn === false) return only(m.contas_nao_conectada());
    if (auth === 'api_key') return only(m.contas_tipo_chave());
    const email = credential.login?.email ?? '';
    const name = credential.apelido || credential.nome_natural || credential.nome;
    const parts = email || name === title ? [] : [name];
    // Sem a causa, o nome sozinho parece login conferido.
    if (naoConferido) parts.push(naoConferido);
    else if (credential.login?.refreshExpiresAt != null) {
      const dias = Math.ceil((credential.login.refreshExpiresAt - Date.now() / 1000) / 86400);
      parts.push(dias > 0 ? m.contas_login_vence({ n: dias }) : m.contas_login_vencido());
    }
    return { email, plan: capitalize(credential.login?.plano), description: parts.join(' · ') };
  };
  // Só conta adicional (~/.codex-<nome>); a padrão é o ~/.codex da máquina e o backend recusa.
  const removeAccount = (account: CodexAccount) => {
    if (!server) return;
    const s = server;
    if (deleting.has(account.id)) return;
    const remove = (keepTranscripts: boolean) => {
      // Segundo toque enquanto a pasta é apagada mandaria outro DELETE da mesma conta.
      if (!alive.current || deletingNow.current.has(account.id)) return;
      deletingNow.current.add(account.id);
      setDeleting((prev) => new Set(prev).add(account.id));
      deleteCodexAccountForServer(s, account.id, keepTranscripts)
        .then((result) => {
          if (!alive.current) return;
          Alert.alert(accountDeletedNotice(account.name, keepTranscripts, result));
          load(s);
        })
        .catch((cause: unknown) => { if (alive.current) setError(cause instanceof Error ? cause.message : m.codex_account_delete_failed()); })
        .finally(() => {
          deletingNow.current.delete(account.id);
          if (!alive.current) return;
          setDeleting((prev) => {
            const next = new Set(prev);
            next.delete(account.id);
            return next;
          });
        });
    };
    // Alerta nativo não tem caixa de marcar: guardar as conversas é o botão preferido.
    Alert.alert(m.contas_apagar_pergunta({ nome: account.name }), m.contas_apagar_opcoes_desc(), [
      { text: m.comum_cancelar(), style: 'cancel' },
      { text: m.contas_apagar_com_conversas(), style: 'destructive', onPress: () => remove(false) },
      { text: m.contas_apagar_juntando(), isPreferred: true, onPress: () => remove(true) },
    ]);
  };
  const agyFor = (credential: Credencial) => credential.tipo === 'antigravity'
    ? agy?.contas.find((account) => account.credencial === credential.id) : undefined;
  // Sem cota nem e-mail: o Hangar não lê o login do Google, só pergunta ao agy se a conta entrou.
  const agyStatus = (account: AntigravityAccount | undefined, loggedIn?: boolean | null) => {
    const parts = [providerName('antigravity'), antigravityLoginText(account ? account.logado : loggedIn)];
    if (account && !account.termos_aceitos) parts.push(m.mobile_antigravity_terms_pending());
    if (account?.em_uso) parts.push(m.mobile_antigravity_in_use());
    if (agy && !agy.instalado) parts.push(m.erro_agy_ausente());
    return { email: '', plan: '', description: parts.join(' · ') };
  };
  const agyCanSignIn = (account: AntigravityAccount | undefined) => !!account && !!agy?.instalado && account.logado !== true;
  const agyBusyKey = (account: AntigravityAccount) => `agy:${account.id}`;
  const agyAction = (account: AntigravityAccount, run: () => Promise<unknown>, failure: string) => {
    if (!server || deletingNow.current.has(agyBusyKey(account))) return;
    const s = server, key = agyBusyKey(account);
    deletingNow.current.add(key);
    setDeleting((prev) => new Set(prev).add(key));
    run()
      .then(() => { if (alive.current) void load(s); })
      .catch((cause: unknown) => { if (alive.current) setError(`${failure}${cause instanceof Error ? `: ${cause.message}` : ''}`); })
      .finally(() => {
        deletingNow.current.delete(key);
        if (!alive.current) return;
        setDeleting((prev) => {
          const next = new Set(prev);
          next.delete(key);
          return next;
        });
      });
  };
  const signOutAgy = (account: AntigravityAccount) => {
    if (!server) return;
    const s = server, nome = antigravityAccountLabel(account);
    Alert.alert(m.contas_sair_pergunta({ nome }), undefined, [
      { text: m.comum_cancelar(), style: 'cancel' },
      { text: m.contas_sair(), style: 'destructive', onPress: () => agyAction(account, () => signOutAntigravityAccountForServer(s, account.id), m.contas_sair_erro()) },
    ]);
  };
  // Só adicional: a pasta inteira sai, com as conversas do agy nela.
  const removeAgy = (account: AntigravityAccount) => {
    if (!server || account.padrao) return;
    const s = server, nome = antigravityAccountLabel(account);
    Alert.alert(m.contas_apagar_pergunta({ nome }), m.contas_apagar_conversas_aviso(), [
      { text: m.comum_cancelar(), style: 'cancel' },
      { text: m.contas_apagar_com_conversas(), style: 'destructive', onPress: () => agyAction(account, () => deleteAntigravityAccountForServer(s, account.id), m.mobile_antigravity_delete_failed()) },
    ]);
  };
  const startLogin = (accountId?: string) => {
    if (!server) return;
    setNewLogin(accountId === undefined);
    setLoginAccount(accountId ?? '');
  };
  const matchedAccountIds = new Set(
    credentials.map(accountForCredential).filter((id): id is CodexAccount => id !== null).map((account) => account.id),
  );
  const built: IdentityRow[] = [
    ...credentials.filter(credential => credential.tipo !== 'cliproxy'
      && !(credential.tipo === 'antigravity' && isAdditionalAntigravity(credential.id, agy?.contas))).map((credential): IdentityRow => {
      const account = accountForCredential(credential);
      const google = agyFor(credential);
      // O nome natural da padrão vem do servidor em português; o apelido dado pela pessoa vence.
      const title = account?.name ?? (google && !credential.apelido ? antigravityAccountLabel(google) : credential.nome ?? credential.nome_natural);
      const group = credentialGroup(credential);
      const status = account ? accountStatus(account, credential)
        : credential.tipo === 'antigravity' ? agyStatus(google, credential.login?.loggedIn) : credentialStatus(credential, title);
      const engineName = credential.tipo === 'chave' && credential.id.startsWith('chave:') ? credential.id.slice(6) : undefined;
      const motor = engineName ? engines.data?.motores[engineName] : undefined;
      const bound = motor?.cliproxy_bound_account;
      const binding = bound ? [bound.email || bound.label, bound.plan].filter(Boolean).join(' · ')
        : motor?.cliproxy_binding || motor?.cliproxy_accounts ? motor.cliproxy_binding_error || m.cliproxy_no_bound_account() : '';
      return {
        key: `credential:${credential.id}`, group, title, ...status,
        description: group === 'claude_engine'
          ? [m.accounts_list_runs_in_claude_code(), proxyNote(engineName), binding || status.description].filter(Boolean).join(' · ')
          : status.description,
        provider: providerOf(credential, account),
        active: credential.ativa,
        account,
        credential,
        engineName,
        antigravity: google ?? null,
        canSignIn: google ? agyCanSignIn(google) : !!account && canSignIn(account),
        subject: { ...credential, nome: title, login: status.email ? { ...credential.login, estado: credential.login?.estado ?? 'ok', email: status.email } : credential.login },
      };
    }),
    ...accounts
      .filter((account) => !matchedAccountIds.has(account.id))
      .map((account): IdentityRow => {
        const status = accountStatus(account);
        return {
          key: `account:${account.id}`, group: groupForAccount(account), title: account.name, ...status,
          provider: 'codex', active: false, account, credential: null, canSignIn: canSignIn(account),
          subject: { tipo: 'codex', nome: account.name, nome_natural: account.name, ativa: false, cota: null,
            login: status.email ? { estado: 'ok', email: status.email } : null },
        };
      }),
    ...(agy?.contas ?? [])
      .filter((google) => google.padrao && !credentials.some((credential) => credential.id === google.credencial))
      .map((google): IdentityRow => {
        const title = antigravityAccountLabel(google);
        return {
          key: `antigravity:${google.id}`, group: 'subscription', title, ...agyStatus(google),
          provider: 'antigravity', active: false, account: null, credential: null, antigravity: google, canSignIn: agyCanSignIn(google),
          subject: { tipo: 'antigravity', nome: title, nome_natural: title, ativa: false, cota: null, login: null },
        };
      }),
    ...(engines.ready ? Object.entries(engines.data!.motores)
      .filter(([name]) => !credentials.some(c => c.id === `chave:${name}`))
      .map(([name, engine]): IdentityRow => ({ key: `engine:${name}`, group: 'claude_engine', title: engine.label || name,
        email: '', plan: '', description: [m.accounts_list_runs_in_claude_code(), proxyNote(name), engine.model, engine.base_url].filter(Boolean).join(' · '),
        provider: '', active: false, account: null, credential: null, engineName: name, canSignIn: false,
        subject: { tipo: 'chave', nome: engine.label || name, nome_natural: name, ativa: false, cota: null, login: null } })) : []),
  ];
  // Linha que não mudou mantém o mesmo objeto: o memo do AccountRow só redesenha a que mudou.
  const nextCache = new Map<string, { sig: string; row: IdentityRow }>();
  const rows = GROUP_ORDER.flatMap((group) => built.filter((row) => row.group === group)).map((row) => {
    const sig = JSON.stringify(row);
    const old = rowCache.current.get(row.key);
    const kept = old?.sig === sig ? old.row : row;
    nextCache.set(row.key, { sig, row: kept });
    return kept;
  });
  rowCache.current = nextCache;
  const counts = Object.fromEntries(ACCOUNT_FILTERS.map((f) => [f, rows.filter((row) => accountInFilter(row.subject, f)).length])) as Record<AccountFilter, number>;
  const visible = sortAccounts(
    rows.filter((row) => accountInFilter(row.subject, filter) && accountMatches(row.subject, term))
      .map((row) => ({ nome: row.subject.nome, cota: row.subject.cota, ativa: row.subject.ativa, row })),
    sort,
  ).map((item) => item.row);
  const windows = [...new Set<string>(credentials.flatMap((c) => c.cota?.estado === 'lida' ? c.cota.janelas.map((w) => w.rotulo) : []))]
    .sort((a, b) => windowRank(a) - windowRank(b));
  const sortOptions: SortOption[] = [
    { key: 'name', label: m.accounts_list_sort_name() },
    ...windows.map((label) => ({ key: label, label: windowLabel(label) })),
    ...(windows.includes('7d') ? [{ key: 'reset', label: m.accounts_list_sort_reset() }] : []),
  ];
  // A ordem salva continua escolhível mesmo quando a janela dela sumiu da lista.
  if (sort && !sortOptions.some((o) => o.key === sort.key)) {
    sortOptions.push({ key: sort.key, label: sort.key === 'reset' ? m.accounts_list_sort_reset() : windowLabel(sort.key) });
  }
  handlersNow.current = {
    signIn: (id) => startLogin(id),
    remove: (account) => removeAccount(account),
    editEngine: (name) => engines.setEditing(name),
    removeEngine: (name) => engines.remove(name),
    refreshClaude: () => refreshClaude(),
    antigravitySignIn: (id) => setAgyLogin({ accountId: id }),
    antigravitySignOut: (account) => signOutAgy(account),
    antigravityRemove: (account) => removeAgy(account),
  };
  // Entrada escalonada só na primeira lista pintada; quem aparece depois (filtro, busca) só surge.
  const firstPaint = !painted.current && visible.length > 0;
  useEffect(() => { if (visible.length) painted.current = true; });
  const actions: PageAction[] = [{ icon: 'RefreshCw', label: m.contas_atualizar(), onPress: () => { load(server); void engines.reload(); void proxy.reload(); } }];
  if (server && engines.ready) actions.push({ label: m.mobile_accounts_engine_new(), onPress: () => engines.setEditing('') });
  // O rótulo já traz o "+"; ícone repetiria o sinal.
  if (server) {
    actions.push({ label: m.contas_nova(), onPress: () => startLogin() });
    actions.push({ label: m.mobile_accounts_claude_add(), onPress: () => setNewClaude(true), disabled: loading });
    // Só a padrão: o atalho existe enquanto ela não aparece na lista (o agy nunca entrou nesta máquina).
    if (agy?.instalado && !agy.contas.some((account) => account.padrao)) {
      actions.push({ label: m.mobile_accounts_antigravity_add(), onPress: () => setAgyLogin({}) });
    }
  }

  return (
    <Pagina>
      <PageHeader title={m.contas_modelos_titulo()} subtitle={m.contas_descricao()} actions={actions} />
      {server ? (
        <SectionCard icon="Server" title={server.label} subtitle={`${m.maquinas_titulo()} · ${server.baseUrl}`} />
      ) : (
        <InfoNotice text={m.maquinas_vazio()} />
      )}
      {server ? <ProxyCard state={proxy} onConnect={() => setCliproxyWizard(true)} /> : null}
      {loading ? <Text style={styles.muted}>{m.comum_carregando()}</Text> : null}
      {error ? <Text accessibilityRole="alert" style={styles.error}>{error}</Text> : null}
      {agyError ? <Text accessibilityRole="alert" style={styles.error}>{m.mobile_antigravity_accounts_error()} {agyError}</Text> : null}
      {server ? <EngineStatus state={engines} /> : null}
      {server ? <ResetFeedback server={server} credentialIds={credentials.map(c => c.id)} onDefinitive={refreshQuota} /> : null}

      {server && rows.length ? (
        <View style={styles.toolbar}>
          <ScrollView horizontal showsHorizontalScrollIndicator={false} keyboardShouldPersistTaps="handled">
            <FilterChips value={filter} counts={counts} onChange={setFilter} />
          </ScrollView>
          <AccountSearch value={term} onChange={setTerm} />
          <View style={styles.sortLine}><SortControl sort={sort} options={sortOptions} onChange={setSort} /></View>
        </View>
      ) : null}
      {server && (rows.length || (!loading && !error && !engines.loading)) ? (
        <SectionCard>
          {visible.map((row, index) => (
            <AccountRow key={row.key} row={row} term={term} enterIndex={firstPaint ? index : null} server={server}
              busy={(!!row.account && deleting.has(row.account.id)) || (!!row.antigravity && deleting.has(agyBusyKey(row.antigravity)))}
              engineReady={!!row.engineName && engines.ready && !!engines.data?.motores[row.engineName]}
              engineBusy={!!row.engineName && engines.deleting.has(row.engineName)} handlers={handlers} />
          ))}
          {!visible.length ? (
            <Text style={styles.empty}>{term.trim() ? m.accounts_list_search_empty({ term: term.trim() }) : m.accounts_list_empty()}</Text>
          ) : null}
        </SectionCard>
      ) : null}
      {visible.some((row) => row.provider === 'antigravity') ? <Text style={styles.muted}>{m.mobile_antigravity_no_quota()}</Text> : null}
      {server && agyLogin ? (
        <AntigravityContaLogin key={JSON.stringify([server.id, server.baseUrl, server.token, agyLogin.accountId ?? ''])} server={server}
          accounts={agy?.contas ?? []} accountId={agyLogin.accountId}
          onComplete={() => { void load(server); }} onClose={() => setAgyLogin(null)} />
      ) : null}
      {server && engines.editing !== null && engines.data ? (
        <Suspense fallback={<Text style={styles.muted}>{m.comum_carregando()}</Text>}>
          <EngineEditor key={engines.editing} server={server} name={engines.editing || undefined}
            engine={engines.editing ? engines.data.motores[engines.editing] : undefined} names={Object.keys(engines.data.motores)}
            onSaved={engines.saved} onClose={() => engines.setEditing(null)}
            onCliproxy={() => { engines.setEditing(null); setCliproxyWizard(true); }} />
        </Suspense>
      ) : null}
      {server && cliproxyWizard ? (
        <Suspense fallback={<Text style={styles.muted}>{m.comum_carregando()}</Text>}>
          <CliproxyWizard server={server} names={Object.keys(engines.data?.motores ?? {})}
            onSaved={engines.saved} onClose={() => { setCliproxyWizard(false); void proxy.reload(); }} />
        </Suspense>
      ) : null}

      {server && newClaude ? (
        <ClaudeAccountCreate server={server} credentials={credentials} onChanged={refreshClaude} onClose={() => setNewClaude(false)} />
      ) : null}
      {selected || newLogin ? (
        <SectionCard
          icon="LogIn"
          title={selected?.name ?? m.novacred_codex_nome()}
          extra={<Pill label={m.login_fechar()} onPress={() => { setLoginAccount(null); setNewLogin(false); }} />}
        >
          {server ? (
            <View style={styles.login}>
              <CodexContaLogin
                server={server}
                accountId={selected?.id}
                onComplete={() => { void load(server); }}
              />
            </View>
          ) : null}
        </SectionCard>
      ) : null}
    </Pagina>
  );
}

const styles = StyleSheet.create((theme) => ({
  muted: { fontSize: theme.base.text.sm, color: theme.tokens.text.muted, paddingHorizontal: theme.base.space[1] },
  error: { fontSize: theme.base.text.sm, color: theme.tokens.status.error, paddingHorizontal: theme.base.space[1] },
  toolbar: { gap: theme.base.space[2] },
  sortLine: { flexDirection: 'row' },
  empty: { fontSize: theme.base.text.sm, color: theme.tokens.text.muted, textAlign: 'center', padding: theme.base.space[6] },
  login: { paddingHorizontal: theme.base.space[4], paddingBottom: theme.base.space[4] },
}));
