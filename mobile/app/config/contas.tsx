import { useCallback, useEffect, useRef, useState } from 'react';
import { Alert, Pressable, Text, View } from 'react-native';
import { StyleSheet } from 'react-native-unistyles';
import { codexCliAusente, credentialAuth, credentialGroup, deleteCodexAccountForServer, getCodexAccountsForServer,
  getCredentialsForServer, type CodexAccount, type Credencial } from '@hangar/core';
import { Pagina } from '../../src/features/config/Pagina';
import { Linha } from '../../src/features/config/Linha';
import { CodexContaLogin } from '../../src/features/config/CodexContaLogin';
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
  const credentialStatus = (credential: Credencial) => {
    const auth = credentialAuth(credential);
    if (auth === 'unknown') return codexCliAusente(credential) ? m.codex_ui_cli_ausente() : m.codex_ui_unknown();
    if (auth === 'none' || credential.login?.loggedIn === false) return m.contas_nao_conectada();
    if (auth === 'api_key') return m.contas_tipo_chave();
    if (credential.login?.email && credential.login.plano) {
      return m.contas_email_plano({ email: credential.login.email, max: credential.login.plano });
    }
    return credential.apelido || credential.nome_natural || credential.nome;
  };
  // Só conta adicional (~/.codex-<nome>); a padrão é o ~/.codex da máquina e o backend recusa.
  const removeAccount = (account: CodexAccount) => {
    if (!server) return;
    const s = server;
    Alert.alert(m.comum_apagar(), account.name, [
      { text: m.comum_cancelar(), style: 'cancel' },
      { text: m.lista_remover(), style: 'destructive', onPress: () => {
        deleteCodexAccountForServer(s, account.id)
          .then(() => load(s))
          .catch((cause: unknown) => setError(cause instanceof Error ? cause.message : m.codex_account_delete_failed()));
      } },
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
    <Linha
      key={identity.key}
      icon="KeyRound"
      titulo={identity.title}
      descricao={identity.description}
      direita={identity.account ? (
        <View style={styles.actions}>
          {identity.account.auth.status !== 'connected' && !codexCliAusente(identity.account) ? (
            <Pressable
              accessibilityRole="button"
              accessibilityLabel={m.contas_entrar()}
              onPress={() => startLogin(identity.account!.id)}
              style={styles.action}
            >
              <Text style={styles.actionText}>{m.contas_entrar()}</Text>
            </Pressable>
          ) : null}
          {!identity.account.is_default ? (
            <Pressable
              accessibilityRole="button"
              accessibilityLabel={m.lista_remover()}
              onPress={() => removeAccount(identity.account!)}
              style={styles.action}
            >
              <Text style={styles.actionText}>{m.lista_remover()}</Text>
            </Pressable>
          ) : null}
        </View>
      ) : null}
    />
  );

  return (
    <Pagina>
      <Text style={styles.title}>{m.contas_titulo()}</Text>
      <Text style={styles.description}>{m.contas_descricao()}</Text>
      <View style={styles.server}>
        <Text style={styles.serverLabel}>{m.maquinas_titulo()}</Text>
        <Text style={styles.serverName}>{server?.label ?? m.maquinas_vazio()}</Text>
        {server ? <Text style={styles.serverUrl}>{server.baseUrl}</Text> : null}
      </View>

      <Linha
        icon="RefreshCw"
        titulo={m.contas_atualizar()}
        onPress={() => load(server)}
      />
      {!server ? <Text style={styles.muted}>{m.maquinas_vazio()}</Text> : null}
      {loading ? <Text style={styles.muted}>{m.comum_carregando()}</Text> : null}
      {error ? <Text accessibilityRole="alert" style={styles.error}>{error}</Text> : null}

      {subscriptions.length ? <Text style={styles.section}>{m.contas_secao_claude()}</Text> : null}
      {subscriptions.map(renderIdentity)}
      {claudeEngines.length ? <Text style={styles.section}>{m.contas_secao_modelos()}</Text> : null}
      {claudeEngines.map(renderIdentity)}
      {apiKeys.length ? <Text style={styles.section}>{m.contas_secao_outros()}</Text> : null}
      {apiKeys.map(renderIdentity)}
      {unknown.length ? <Text style={styles.section}>{m.codex_ui_unknown()}</Text> : null}
      {unknown.map(renderIdentity)}

      {server ? (
        <Linha icon="Plus" titulo={m.contas_nova()} onPress={() => startLogin()} />
      ) : null}

      {selected || newLogin ? (
        <View style={styles.login}>
          <View style={styles.loginHeader}>
            <Text style={styles.loginTitle}>{selected?.name ?? m.novacred_codex_nome()}</Text>
            <Pressable accessibilityRole="button" accessibilityLabel={m.login_fechar()} onPress={() => { setLoginAccount(null); setNewLogin(false); }}>
              <Text style={styles.close}>{m.login_fechar()}</Text>
            </Pressable>
          </View>
          {server ? (
            <CodexContaLogin
              server={server}
              accountId={selected?.id}
              onComplete={() => { void load(server); }}
            />
          ) : null}
        </View>
      ) : null}
    </Pagina>
  );
}

const styles = StyleSheet.create((theme) => ({
  title: { fontSize: theme.base.text.xl, fontWeight: '600', color: theme.tokens.text.primary },
  description: { fontSize: theme.base.text.sm, color: theme.tokens.text.secondary },
  server: { gap: 2, padding: theme.base.space[3], borderRadius: theme.base.radius.md, backgroundColor: theme.tokens.bg.surface },
  serverLabel: { fontSize: theme.base.text.xs, color: theme.tokens.text.muted },
  serverName: { fontSize: theme.base.text.base, fontWeight: '600', color: theme.tokens.text.primary },
  serverUrl: { fontFamily: theme.base.fontMono, fontSize: theme.base.text.xs, color: theme.tokens.text.muted },
  muted: { fontSize: theme.base.text.sm, color: theme.tokens.text.muted },
  error: { fontSize: theme.base.text.sm, color: theme.tokens.status.error },
  actions: { flexDirection: 'row', alignItems: 'center' },
  action: { minHeight: 44, justifyContent: 'center', paddingHorizontal: theme.base.space[2] },
  actionText: { color: theme.tokens.accent.base, fontSize: theme.base.text.xs, fontWeight: '600' },
  login: { gap: theme.base.space[3], padding: theme.base.space[3], borderRadius: theme.base.radius.lg, backgroundColor: theme.tokens.bg.surface },
  loginHeader: { flexDirection: 'row', alignItems: 'center', justifyContent: 'space-between', gap: theme.base.space[2] },
  loginTitle: { flex: 1, fontSize: theme.base.text.lg, fontWeight: '600', color: theme.tokens.text.primary },
  close: { color: theme.tokens.accent.base, fontSize: theme.base.text.sm },
  section: { fontSize: theme.base.text.sm, fontWeight: '600', color: theme.tokens.text.secondary, paddingTop: theme.base.space[2] },
}));
