import { useCallback, useEffect, useRef, useState } from 'react';
import { Alert, Text, View } from 'react-native';
import { StyleSheet } from 'react-native-unistyles';
import { codexCliAusente, credentialAuth, credentialGroup, deleteCodexAccountForServer, getCodexAccountsForServer,
  getCredentialsForServer, accountDeletedNotice, type CodexAccount, type Credencial } from '@hangar/core';
import { Pagina } from '../../src/features/config/Pagina';
import { PageHeader, Pill, type PageAction } from '../../src/features/config/PageHeader';
import { SectionCard } from '../../src/features/config/SectionCard';
import { SettingsRow } from '../../src/features/config/SettingsRow';
import { InfoNotice } from '../../src/features/config/InfoNotice';
import { CodexContaLogin } from '../../src/features/config/CodexContaLogin';
import type { IconName } from '../../src/ui/Icon';
import { useServers } from '../../src/stores/servers';
import * as m from '../../src/paraglide/messages';

type CredentialGroup = ReturnType<typeof credentialGroup>;
type IdentityRow = { key: string; group: CredentialGroup; title: string; description: string; account: CodexAccount | null };

export default function Contas() {
  const server = useServers((state) => state.active());
  const [accounts, setAccounts] = useState<CodexAccount[]>([]);
  const [credentials, setCredentials] = useState<Credencial[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState('');
  const [loginAccount, setLoginAccount] = useState<string | null>(null);
  const [newLogin, setNewLogin] = useState(false);
  const [deleting, setDeleting] = useState<ReadonlySet<string>>(new Set());
  const generation = useRef(0);
  const controller = useRef<AbortController | null>(null);

  const load = useCallback((target: typeof server) => {
    const generationNow = ++generation.current;
    controller.current?.abort();
    const nextController = new AbortController();
    controller.current = nextController;
    setAccounts([]);
    setCredentials([]);
    setError('');
    if (!target) {
      setLoading(false);
      return;
    }
    setLoading(true);
    void Promise.all([
      getCodexAccountsForServer(target, nextController.signal),
      getCredentialsForServer(target, false, nextController.signal),
    ])
      .then(([next, nextCredentials]) => {
        if (generationNow !== generation.current || nextController.signal.aborted) return;
        setAccounts(next);
        setCredentials(nextCredentials);
      })
      .catch((cause: unknown) => {
        if (generationNow !== generation.current || nextController.signal.aborted) return;
        setError(cause instanceof Error ? cause.message : m.codex_ui_login_error());
      })
      .finally(() => {
        if (generationNow === generation.current && !nextController.signal.aborted) setLoading(false);
      });
  }, []);

  useEffect(() => {
    setLoginAccount(null);
    setNewLogin(false);
    load(server);
    return () => {
      controller.current?.abort();
      generation.current++;
    };
  }, [load, server?.id, server?.baseUrl, server?.token]);

  const selected = loginAccount === null ? null : accounts.find((account) => account.id === loginAccount) ?? null;
  const accountForCredential = (credential: Credencial) => {
    if (credential.tipo !== 'codex') return null;
    return accounts.find((account) => account.id === credential.codex_account || account.credential_id === credential.id) ?? null;
  };
  const accountStatus = (account: CodexAccount) => {
    const auth = account.auth;
    return auth.status === 'connected'
      ? (auth.email && auth.plan ? m.contas_email_plano({ email: auth.email, max: auth.plan }) : auth.email ?? m.codex_ui_account())
      : auth.status === 'disconnected' ? m.contas_nao_conectada()
      : codexCliAusente(account) ? m.codex_ui_cli_ausente() : m.codex_ui_unknown();
  };
  const groupForAccount = (account: CodexAccount): CredentialGroup => (
    account.auth.method === 'oauth' ? 'subscription' : account.auth.method === 'api_key' ? 'api_key' : 'unknown'
  );
  const canSignIn = (account: CodexAccount) => account.auth.status !== 'connected' && !codexCliAusente(account);
  const credentialStatus = (credential: Credencial) => {
    const auth = credentialAuth(credential);
    if (auth === 'unknown') return codexCliAusente(credential) ? m.codex_ui_cli_ausente() : m.codex_ui_unknown();
    if (auth === 'none' || credential.login?.loggedIn === false) return m.contas_nao_conectada();
    if (auth === 'api_key') return m.contas_tipo_chave();
    const base = credential.login?.email && credential.login.plano
      ? m.contas_email_plano({ email: credential.login.email, max: credential.login.plano })
      : credential.apelido || credential.nome_natural || credential.nome;
    const exp = credential.login?.refreshExpiresAt;
    if (exp == null) return base;
    const dias = Math.ceil((exp - Date.now() / 1000) / 86400);
    return `${base} · ${dias > 0 ? m.contas_login_vence({ n: dias }) : m.contas_login_vencido()}`;
  };
  // Só conta adicional (~/.codex-<nome>); a padrão é o ~/.codex da máquina e o backend recusa.
  const removeAccount = (account: CodexAccount) => {
    if (!server) return;
    const s = server;
    if (deleting.has(account.id)) return;
    const remove = (keepTranscripts: boolean) => {
      // Segundo toque enquanto a pasta é apagada mandaria outro DELETE da mesma conta.
      setDeleting((prev) => new Set(prev).add(account.id));
      deleteCodexAccountForServer(s, account.id, keepTranscripts)
        .then((result) => {
          Alert.alert(accountDeletedNotice(account.name, keepTranscripts, result));
          load(s);
        })
        .catch((cause: unknown) => setError(cause instanceof Error ? cause.message : m.codex_account_delete_failed()))
        .finally(() => setDeleting((prev) => {
          const next = new Set(prev);
          next.delete(account.id);
          return next;
        }));
    };
    // Alerta nativo não tem caixa de marcar: guardar as conversas é o botão preferido.
    Alert.alert(m.contas_apagar_pergunta({ nome: account.name }), m.contas_apagar_opcoes_desc(), [
      { text: m.comum_cancelar(), style: 'cancel' },
      { text: m.contas_apagar_com_conversas(), style: 'destructive', onPress: () => remove(false) },
      { text: m.contas_apagar_juntando(), isPreferred: true, onPress: () => remove(true) },
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
  const identities: IdentityRow[] = [
    ...credentials.map((credential): IdentityRow => {
      const account = accountForCredential(credential);
      return {
        key: `credential:${credential.id}`,
        group: credentialGroup(credential),
        title: account?.name ?? credential.nome ?? credential.nome_natural,
        description: account ? accountStatus(account) : credentialStatus(credential),
        account,
      };
    }),
    ...accounts
      .filter((account) => !matchedAccountIds.has(account.id))
      .map((account): IdentityRow => ({
        key: `account:${account.id}`,
        group: groupForAccount(account),
        title: account.name,
        description: accountStatus(account),
        account,
      })),
  ];
  const subscriptions = identities.filter((identity) => identity.group === 'subscription');
  const claudeEngines = identities.filter((identity) => identity.group === 'claude_engine');
  const apiKeys = identities.filter((identity) => identity.group === 'api_key');
  const unknown = identities.filter((identity) => identity.group === 'unknown');
  const renderIdentity = (identity: IdentityRow) => (
    <SettingsRow key={identity.key} icon="KeyRound" title={identity.title} description={identity.description}>
      {identity.account && (canSignIn(identity.account) || !identity.account.is_default) ? (
        <View style={styles.actions}>
          {canSignIn(identity.account) ? (
            <Pill icon="LogIn" label={m.contas_entrar()} onPress={() => startLogin(identity.account!.id)}
              disabled={deleting.has(identity.account.id)} />
          ) : null}
          {!identity.account.is_default ? (
            <Pill icon="Trash2" label={m.lista_remover()} onPress={() => removeAccount(identity.account!)}
              disabled={deleting.has(identity.account.id)} />
          ) : null}
        </View>
      ) : null}
    </SettingsRow>
  );
  // Seção vazia não aparece, e o título vai junto.
  const section = (icon: IconName, title: string, subtitle: string, rows: IdentityRow[]) =>
    rows.length ? <SectionCard icon={icon} title={title} subtitle={subtitle}>{rows.map(renderIdentity)}</SectionCard> : null;
  const actions: PageAction[] = [{ icon: 'RefreshCw', label: m.contas_atualizar(), onPress: () => load(server) }];
  // O rótulo já traz o "+"; ícone repetiria o sinal.
  if (server) actions.push({ label: m.contas_nova(), onPress: () => startLogin() });

  return (
    <Pagina>
      <PageHeader title={m.contas_modelos_titulo()} subtitle={m.contas_descricao()} actions={actions} />
      {server ? (
        <SectionCard icon="Server" title={server.label} subtitle={`${m.maquinas_titulo()} · ${server.baseUrl}`} />
      ) : (
        <InfoNotice text={m.maquinas_vazio()} />
      )}
      {loading ? <Text style={styles.muted}>{m.comum_carregando()}</Text> : null}
      {error ? <Text accessibilityRole="alert" style={styles.error}>{error}</Text> : null}

      {section('CreditCard', m.contas_secao_claude(), m.contas_secao_claude_leg(), subscriptions)}
      {section('Cpu', m.contas_secao_modelos(), m.contas_secao_modelos_leg(), claudeEngines)}
      {section('KeyRound', m.contas_secao_outros(), m.contas_secao_outros_leg(), apiKeys)}
      {section('CircleHelp', m.codex_ui_unknown(), '', unknown)}

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
  muted: { fontSize: theme.base.text.sm, color: theme.tokens.text.muted, paddingHorizontal: 4 },
  error: { fontSize: theme.base.text.sm, color: theme.tokens.status.error, paddingHorizontal: 4 },
  actions: { flexDirection: 'row', flexWrap: 'wrap', gap: 8 },
  login: { paddingHorizontal: 16, paddingBottom: 16 },
}));
