import { useEffect, useRef, useState } from 'react';
import { ActivityIndicator, Alert, Pressable, ScrollView, Text, View } from 'react-native';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { fetchSessionsForServer, formataErro, getCredentialsForServer, getEnginesForServer, listAccountTargets, setSessionAccount, setSessionEngineAccount, type Credencial, type Server } from '@hangar/core';
import { chatStore } from '../stores/chat';
import { useServers } from '../stores/servers';
import { useSessions } from '../stores/sessions';
import { sessionAccountLabel } from './sessionAccountLabel';
import { accountOptions, isCurrentAccount, sessionCredential, type AccountSession, type SwitchTarget } from './sessionAccountOptions';
import { SessionAccountCard } from './SessionAccountCard';
import { Icon } from '../ui/Icon';
import * as m from '../paraglide/messages';

type Catalog = { identity: string; loading: boolean; error: string; targets: SwitchTarget[]; credentials?: Credencial[]; restricted?: boolean };
type AccountTransition = { scope: string; source: AccountSession; destination: AccountSession };
type Feedback = { identity: string; error: string; success: string; transition?: AccountTransition };
const normalizedPath = (path?: string | null) => path?.replace(/\\/g, '/');
function movedTranscript(jsonl: string | null | undefined, configDir: string) {
  const parts = normalizedPath(jsonl)?.match(/\/projects\/([^/]+)\/([^/]+\.jsonl)$/);
  return parts ? `${normalizedPath(configDir)?.replace(/\/$/, '')}/projects/${parts[1]}/${parts[2]}` : jsonl;
}
function matchesTransition(transition: AccountTransition, scope: string, session: AccountSession | null) {
  if (scope !== transition.scope || !session) return false;
  const matches = (expected: AccountSession) => session.provider === expected.provider
    && session.engine === expected.engine && session.guest_kind === expected.guest_kind
    && session.conta === expected.conta && normalizedPath(session.jsonl) === normalizedPath(expected.jsonl)
    && (session.config_dir == null || normalizedPath(session.config_dir) === normalizedPath(expected.config_dir ?? expected.conta?.replace(/^claude:/, '')))
    && (session.engine_account == null || session.engine_account === expected.engine_account);
  return matches(transition.source) || matches(transition.destination)
    || matches({ ...transition.destination, jsonl: transition.source.jsonl });
}
const readableError = (error: unknown) => (error instanceof Error ? error.message : formataErro(error)) || m.native_invalid_response();
const serverIdentity = (serverId: string, name: string, server?: Server) => JSON.stringify([
  serverId, name, server?.baseUrl, server?.token, server?.removed, server?.invite, server?.inviteEnded,
]);
const sessionIdentity = (scope: string, session: AccountSession | null) => JSON.stringify([
  scope, session?.jsonl, session?.provider, session?.engine, session?.conta,
  session?.config_dir, session?.engine_account, session?.guest_kind, session?.lifecycle_id,
]);

export function SessionAccount({ serverId, name }: { serverId: string; name: string }) {
  const { theme } = useUnistyles();
  const insets = useSafeAreaInsets();
  const ready = useServers(s => s.ready);
  const server = useServers(s => s.servers.find(x => x.id === serverId));
  const row = useSessions(s => s.rows.find(x => x.serverId === serverId && x.name === name));
  const bucket = useSessions(s => s.byServer.find(x => x.server.id === serverId));
  const listed = !!bucket?.loaded && !bucket.error;
  const scope = serverIdentity(serverId, name, server);
  const [fallback, setFallback] = useState<{ scope: string; session: AccountSession | null } | null>(null);
  const [sessionFailure, setSessionFailure] = useState({ scope: '', error: '' });
  const sessionError = sessionFailure.scope === scope ? sessionFailure.error : '';
  const [retry, setRetry] = useState(0);
  const session: AccountSession | null = row ?? (!listed && fallback?.scope === scope ? fallback.session : null);
  const refused = !server || !!(server.removed || server.invite || server.inviteEnded);
  const identity = sessionIdentity(scope, session);
  const mounted = useRef(false);
  const liveContext = () => {
    const liveServer = useServers.getState().servers.find(x => x.id === serverId);
    const liveScope = serverIdentity(serverId, name, liveServer);
    const sessions = useSessions.getState();
    const liveBucket = sessions.byServer.find(x => x.server.id === serverId);
    const liveListed = !!liveBucket?.loaded && !liveBucket.error;
    const liveSession: AccountSession | null = sessions.rows.find(x => x.serverId === serverId && x.name === name)
      ?? (!liveListed && fallback?.scope === liveScope ? fallback.session : null);
    return { scope: liveScope, session: liveSession, identity: sessionIdentity(liveScope, liveSession) };
  };
  const isCurrent = () => mounted.current && useServers.getState().ready && liveContext().identity === identity;
  const [catalog, setCatalog] = useState<Catalog>({ identity: '', loading: true, error: '', targets: [] });
  const current = catalog.identity === identity ? catalog : { identity, loading: true, error: '', targets: [] };

  useEffect(() => {
    mounted.current = true;
    const release = useSessions.getState().retain();
    return () => { mounted.current = false; release(); };
  }, []);
  useEffect(() => {
    if (!ready || refused || !server || row || listed) return;
    let active = true;
    const valid = () => active && mounted.current && liveContext().scope === scope;
    setFallback(null);
    setSessionFailure({ scope, error: '' });
    void fetchSessionsForServer(server).then(rows => {
      if (!valid()) return;
      const found = rows.find(x => x.name === name) ?? null;
      setFallback({ scope, session: found });
      if (!found) setSessionFailure({ scope, error: m.chat_sessao_encerrada() });
    }).catch(error => { if (valid()) setSessionFailure({ scope, error: readableError(error) }); });
    return () => { active = false; };
  }, [ready, refused, scope, !!row, listed, bucket?.error, retry]);

  useEffect(() => {
    if (!ready || refused || !server || !session || session.provider !== 'claude' || session.guest_kind === 'pair') return;
    let active = true;
    setCatalog({ identity, loading: true, error: '', targets: [] });
    const load = async (): Promise<Pick<Catalog, 'targets' | 'credentials' | 'restricted'>> => {
      if (!session.engine) {
        const [targets, credentials] = await Promise.all([listAccountTargets(name, server), getCredentialsForServer(server)]);
        return { targets: accountOptions(targets, credentials), credentials };
      }
      const engines = await getEnginesForServer(server);
      const engine = engines.motores[session.engine];
      if (engines.arquivo_corrompido || !engine) throw new Error(m.native_invalid_response());
      if (engine.cliproxy_error) throw new Error(engine.cliproxy_error);
      if (!engine.cliproxy_accounts?.length) return { targets: [], restricted: true };
      const credentials = await getCredentialsForServer(server);
      const targets: SwitchTarget[] = [];
      for (const account of engine.cliproxy_accounts) {
        if (!account.account.trim()) continue;
        // Conta só do proxy: sem credencial Codex no Hangar e sem cota.
        if (/^cliproxy:./.test(account.credential_id)) {
          targets.push({ path: account.credential_id, engineAccount: account.account,
            label: account.label || account.email || account.account, pct: null, full: false, low: false });
          continue;
        }
        if (!/^codex:./.test(account.credential_id)) continue;
        const credential = credentials.find(c => c.id === account.credential_id && c.tipo === 'codex');
        if (!credential) continue;
        const windows = credential.cota?.estado === 'lida' ? credential.cota.janelas.filter(w => Number.isFinite(w.pct)).map(w => w.pct) : [];
        const pct = windows.length ? Math.max(...windows) : null;
        targets.push({ path: account.credential_id, engineAccount: account.account,
          label: account.label || account.email || account.account, pct, full: pct !== null && pct >= 99, low: pct !== null && pct >= 95 });
      }
      return { targets: accountOptions(targets, credentials), credentials };
    };
    void load().then(result => {
      if (active && isCurrent()) setCatalog({ identity, loading: false, error: '', ...result });
    }).catch(error => {
      if (active && isCurrent()) setCatalog({ identity, loading: false, error: readableError(error), targets: [] });
    });
    return () => { active = false; };
  }, [ready, identity, !!session, retry]);

  const chat = chatStore(serverId, name);
  const stateEvent = chat.use(s => s.stateEvent);
  const idle = (stateEvent?.state ?? session?.state) === 'idle';
  const [applying, setApplying] = useState(false);
  const [confirming, setConfirming] = useState(false);
  const operation = useRef<{ phase: 'confirm' | 'post' } | null>(null);
  const [feedback, setFeedback] = useState<Feedback>({ identity: '', error: '', success: '' });
  const feedbackMatches = feedback.transition ? matchesTransition(feedback.transition, scope, session) : feedback.identity === identity;
  const actionError = feedbackMatches ? feedback.error : '';
  const success = feedbackMatches ? feedback.success : '';
  const displayedSession = success && feedback.transition ? feedback.transition.destination : session;
  const label = displayedSession ? sessionAccountLabel(displayedSession) : null;
  const activeTarget = displayedSession ? current.targets.find(target => isCurrentAccount(target, displayedSession)) : undefined;
  const activeCredential = displayedSession ? sessionCredential(displayedSession, 'credentials' in current ? current.credentials ?? [] : []) ?? activeTarget?.credential : undefined;
  const shownTargets = current.targets.filter(target => !displayedSession || !isCurrentAccount(target, displayedSession));
  const requestSwitch = (target: SwitchTarget) => {
    if (!server || refused || !session || session.provider !== 'claude' || session.guest_kind === 'pair'
      || !idle || operation.current || target.full || target.login === false || !isCurrent()) return;
    const pending = { phase: 'confirm' as 'confirm' | 'post' };
    operation.current = pending;
    setConfirming(true);
    const release = () => {
      if (operation.current !== pending) return;
      operation.current = null;
      if (mounted.current) { setConfirming(false); setApplying(false); }
    };
    const cancel = () => { if (pending.phase === 'confirm') release(); };
    const execute = async () => {
      if (operation.current !== pending || pending.phase !== 'confirm') return;
      pending.phase = 'post';
      if (!isCurrent()) {
        if (mounted.current) setFeedback({ identity: liveContext().identity, error: m.mobile_session_account_changed(), success: '' });
        release(); return;
      }
      if ((chat.use.getState().stateEvent?.state ?? liveContext().session?.state) !== 'idle') {
        setFeedback({ identity, error: m.native_usage_card_move_busy(), success: '' });
        release(); return;
      }
      const next: AccountSession = target.engineAccount
        ? { ...session, conta: target.path, engine_account: target.engineAccount }
        : { ...session, conta: `claude:${target.path}`, config_dir: target.path, jsonl: movedTranscript(session.jsonl, target.path) };
      const transition: AccountTransition = { scope, source: session, destination: next };
      const resultIsCurrent = () => {
        const live = liveContext();
        return mounted.current && useServers.getState().ready && matchesTransition(transition, live.scope, live.session);
      };
      setConfirming(false); setApplying(true);
      setFeedback({ identity, error: '', success: '' });
      try {
        if (target.engineAccount) {
          const result = await setSessionEngineAccount(name, target.engineAccount, server);
          if (!result.ok) throw new Error(m.native_invalid_response());
        } else {
          const result = await setSessionAccount(name, target.path, server);
          if (!result.ok || result.config_dir !== target.path) throw new Error(m.native_invalid_response());
        }
        if (!resultIsCurrent()) return;
        setFeedback({ identity, transition, error: '', success: m.mobile_session_account_success({ conta: sessionAccountLabel(next) || target.label }) });
      } catch (error) {
        if (resultIsCurrent()) setFeedback({ identity, transition, error: readableError(error), success: '' });
      } finally { release(); }
    };
    const warning = target.low && target.pct !== null ? `\n\n${m.conta_confirmar_acabando({ conta: target.label, pct: target.pct })}` : '';
    Alert.alert(m.conta_confirmar_titulo({ conta: target.label }), m.conta_confirmar_msg({ conta: target.label }) + warning,
      [{ text: m.comum_cancelar(), style: 'cancel', onPress: cancel }, { text: m.comum_confirmar(), onPress: () => { void execute(); } }],
      { cancelable: true, onDismiss: cancel });
  };
  const loading = <View style={styles.notice}><ActivityIndicator color={theme.tokens.text.muted} /><Text style={styles.text}>{m.comum_carregando()}</Text></View>;
  const failure = (text: string) => <View style={styles.notice}>
    <Text accessibilityRole="alert" style={styles.error}>{text}</Text>
    <Pressable accessibilityRole="button" accessibilityLabel={m.native_retry()} onPress={() => setRetry(n => n + 1)} style={styles.button}>
      <Text style={styles.action}>{m.native_retry()}</Text>
    </Pressable>
  </View>;
  return <ScrollView style={styles.scroll} contentInsetAdjustmentBehavior="never" contentContainerStyle={[styles.content, { paddingBottom: theme.base.space[8] + insets.bottom }]}>
    {!ready ? loading : refused ? <Text accessibilityRole="alert" style={styles.error}>{m.chat_servidor_removido()}</Text> : <>
      <SessionAccountCard current label={activeTarget?.label || activeCredential?.apelido || activeCredential?.nome || label || m.native_invalid_response()}
        context={displayedSession?.engine ? label : null} credential={activeCredential} path={displayedSession?.conta?.replace(/^(claude|codex):/, '')} />
      {!session ? sessionError ? failure(sessionError) : listed ? <Text style={styles.error}>{m.chat_sessao_encerrada()}</Text> : bucket?.error ? failure(bucket.error) : loading
        : session.provider !== 'claude' || session.guest_kind === 'pair' ? <Text style={styles.text}>{m.conta_mesma_so_claude()}</Text>
          : <>
            <View style={styles.explanation}><Icon name="ArrowLeftRight" size={18} color={theme.tokens.text.muted} /><Text style={[styles.text, styles.grow]}>{m.conta_mesma_conversa_sub()}</Text></View>
            {!idle ? <Text style={styles.text}>{m.native_usage_card_move_busy()}</Text> : null}
            {applying ? loading : null}
            {actionError ? <Text accessibilityRole="alert" style={styles.error}>{actionError}</Text> : null}
            {success ? <Text accessibilityRole="alert" style={styles.text}>{success}</Text> : null}
            {current.loading ? loading : current.error ? failure(m.conta_erro_listar({ erro: current.error }))
              : current.restricted ? <Text style={styles.text}>{m.mobile_session_engine_account_unavailable()}</Text>
                : shownTargets.length === 0 ? <Text style={styles.text}>{session.engine ? m.accounts_none() : m.conta_nenhuma_outra()}</Text>
                : <>
                  <Text accessibilityRole="header" style={styles.caption}>{m.mobile_account_order()}</Text>
                  {shownTargets.map(target => <SessionAccountCard key={target.path} label={target.label} credential={target.credential} path={target.path}
                    pct={target.pct} full={target.full} unconfirmed={target.unconfirmed} noLogin={target.login === false}
                    disabled={target.full || target.login === false || !idle || applying || confirming} onPress={() => requestSwitch(target)} />)}
                </>}
          </>}
    </>}
  </ScrollView>;
}

const styles = StyleSheet.create(theme => ({
  scroll: { flex: 1 },
  grow: { flex: 1 },
  explanation: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2], paddingVertical: theme.base.space[2] },
  content: { padding: theme.base.space[4], paddingBottom: theme.base.space[8], gap: theme.base.space[3] },
  notice: { gap: theme.base.space[2] },
  caption: { fontSize: theme.base.text.sm, color: theme.tokens.text.muted },
  text: { fontSize: theme.base.text.sm, color: theme.tokens.text.secondary },
  error: { fontSize: theme.base.text.sm, color: theme.tokens.status.error },
  action: { fontSize: theme.base.text.base, color: theme.tokens.accent.base },
  button: { minHeight: 44, paddingVertical: theme.base.space[3], gap: theme.base.space[1] },
}));
