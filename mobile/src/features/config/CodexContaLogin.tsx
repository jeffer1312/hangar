import { useEffect, useRef, useState } from 'react';
import { AppState, Pressable, Text, TextInput, View } from 'react-native';
import { StyleSheet } from 'react-native-unistyles';
import * as Linking from 'expo-linking';
import * as Clipboard from 'expo-clipboard';
import { createCodexAccountForServer, prepareCodexAccountForServer, getCodexPreparationForServer,
  startCodexAccountLoginForServer, getCodexAccountLoginForServer, cancelCodexAccountLoginForServer,
  getCodexAccountsForServer, codexAccountMessage, contaCodexParaEntrar,
  type Server, type CodexAccount, type CodexLoginAttempt } from '@hangar/core';
import * as m from '../../paraglide/messages';

export function CodexContaLogin({ server, accountId, onComplete }: {
  server: Server; accountId?: string; onComplete: () => void;
}) {
  const [name, setName] = useState('');
  const [account, setAccount] = useState(accountId);
  const [attempt, setAttempt] = useState<CodexLoginAttempt | null>(null);
  const [sync, setSync] = useState<CodexAccount['sync'] | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const generation = useRef(0);
  const controller = useRef(new AbortController());
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const complete = useRef(onComplete);
  complete.current = onComplete;
  const fail = (e: unknown) => e instanceof Error ? e.message : m.codex_ui_login_error();
  const stop = () => { clearTimeout(timer.current); controller.current.abort(); controller.current = new AbortController(); return ++generation.current; };

  function show(next: CodexLoginAttempt | null) {
    setAttempt(next);
    if (next?.status === 'completed') complete.current();
  }
  async function read(s: Server, id: string, g: number, signal: AbortSignal) {
    clearTimeout(timer.current);
    try {
      const next = await getCodexAccountLoginForServer(s, id, signal);
      if (g !== generation.current) return;
      setError(''); show(next);
      if (next?.status === 'waiting') timer.current = setTimeout(() => void read(s, id, g, signal), 1000);
    } catch (e) { if (g === generation.current) setError(fail(e)); }
  }
  // "Adicionar conta" com a padrao ainda sem login = entrar NELA, sem nome nem conta extra.
  async function entrarNaPadrao(s: Server, g: number, signal: AbortSignal) {
    try {
      const id = contaCodexParaEntrar(await getCodexAccountsForServer(s, signal));
      if (g !== generation.current || !id) return;
      setAccount(id);
      void read(s, id, g, signal);
    } catch { /* lista indisponivel: segue criando conta nomeada, como antes */ }
  }
  useEffect(() => {
    const g = stop(), signal = controller.current.signal;
    setAccount(accountId); setAttempt(null); setSync(null); setBusy(false); setError(''); setName('');
    if (accountId) void read(server, accountId, g, signal);
    else void entrarNaPadrao(server, g, signal);
    return () => { stop(); };
  }, [server.id, server.baseUrl, server.token, accountId]);
  useEffect(() => {
    const listener = AppState.addEventListener('change', state => {
      if (state === 'active' && account && !busy && attempt?.status === 'waiting') {
        const g = stop();
        void read(server, account, g, controller.current.signal);
      }
    });
    return () => listener.remove();
  }, [server, account, busy, attempt?.status]);

  async function start() {
    if (busy) return;
    const s = server, g = stop(), signal = controller.current.signal;
    setBusy(true); setError('');
    try {
      let id = account;
      if (!id) {
        const created = await createCodexAccountForServer(s, name.trim());
        if (g !== generation.current) return;
        id = created.id; setAccount(id);
      }
      const prepared = await prepareAccount(s, id, g, signal);
      if (!prepared || g !== generation.current) return;
      if (prepared.status !== 'ready') return;
      const next = await startCodexAccountLoginForServer(s, id);
      if (g !== generation.current) return;
      show(next);
      if (next.status === 'waiting') timer.current = setTimeout(() => void read(s, id, g, signal), 1000);
    } catch (e) { if (g === generation.current) setError(fail(e)); }
    finally { if (g === generation.current) setBusy(false); }
  }
  async function prepareAccount(s: Server, id: string, g: number, signal: AbortSignal, force = false) {
    let prepared = force
      ? await prepareCodexAccountForServer(s, id, true)
      : await prepareCodexAccountForServer(s, id);
    if (g !== generation.current) return null;
    setSync(prepared);
    while (prepared.status === 'running') {
        await new Promise<void>(resolve => {
          const done = () => { clearTimeout(t); signal.removeEventListener('abort', done); resolve(); };
          const t = setTimeout(done, 1000); signal.addEventListener('abort', done, { once: true });
        });
        if (g !== generation.current) return;
        prepared = await getCodexPreparationForServer(s, id, signal);
        if (g !== generation.current) return;
        setSync(prepared);
    }
    return prepared;
  }
  async function reconcile() {
    if (!account || busy) return;
    const s = server, id = account, g = stop(), signal = controller.current.signal;
    setBusy(true); setError('');
    try {
      const prepared = await prepareAccount(s, id, g, signal, true);
      if (prepared && prepared.status !== 'ready') {
        setError(prepared.issues.map(codexAccountMessage).join('\n') || m.codex_ui_prepare_error());
      }
    } catch (e) { if (g === generation.current) setError(fail(e)); }
    finally { if (g === generation.current) setBusy(false); }
  }
  async function cancel() {
    if (!account || !attempt || busy) return;
    const s = server, id = account, attemptId = attempt.attempt_id, g = stop();
    setBusy(true); setError('');
    try {
      const next = await cancelCodexAccountLoginForServer(s, id, attemptId);
      if (g === generation.current) show(next);
    } catch (e) { if (g === generation.current) setError(fail(e)); }
    finally { if (g === generation.current) setBusy(false); }
  }
  async function external(copy: boolean) {
    const g = generation.current;
    try {
      if (copy && attempt?.user_code) await Clipboard.setStringAsync(attempt.user_code);
      else {
        const url = new URL(attempt?.verification_url ?? '');
        if (url.protocol !== 'https:' || !url.hostname || url.username || url.password) throw new Error(m.codex_ui_login_error());
        await Linking.openURL(url.href);
      }
    } catch (e) { if (g === generation.current) setError(fail(e)); }
  }
  const button = (label: string, action: () => void, disabled = busy) => (
    <Pressable accessibilityRole="button" accessibilityLabel={label} disabled={disabled} onPress={action} style={styles.button}><Text style={styles.text}>{label}</Text></Pressable>
  );
  return <View style={styles.root}>
    <Text style={styles.text}>{m.codex_ui_oauth()}</Text>
    {!account ? <TextInput accessibilityLabel={m.novacred_nome_conta()} placeholder={m.novacred_nome_conta()} value={name} onChangeText={setName} editable={!busy} autoCapitalize="none" style={styles.input} /> : null}
    {sync ? <Text style={styles.text}>{sync.status === 'running' ? m.codex_ui_preparing() : sync.status === 'ready' ? m.codex_ui_inherited() : m.codex_ui_prepare_error()}</Text> : null}
    {sync?.trust_pending ? <Text style={styles.text}>{m.harness_codex_confianca()}</Text> : null}
    {sync?.issues.map((issue, i) => <Text accessibilityRole="alert" style={styles.error} key={i}>{codexAccountMessage(issue)}</Text>)}
    {attempt?.status === 'waiting' ? <>
      <Text style={styles.text}>{m.novacred_codex_aguardando()}</Text>
      {attempt.user_code ? <><Text selectable style={styles.text}>{attempt.user_code}</Text>{button(m.comum_copiar_codigo(), () => void external(true))}</> : null}
      {attempt.verification_url ? button(m.term_abrir_link(), () => void external(false)) : null}
      {button(m.codex_ui_cancel_login(), () => void cancel())}
      {error ? button(m.novacred_codex_tentar(), () => { if (account) { const g = stop(); void read(server, account, g, controller.current.signal); } }) : null}
    </> : attempt?.status === 'completed' ? <Text style={styles.text}>{m.novacred_codex_concluido()}</Text> : <>
      {attempt?.error ? <Text accessibilityRole="alert" style={styles.error}>{codexAccountMessage(attempt.error)}</Text> : null}
      {attempt?.status === 'cancelled' ? <Text style={styles.text}>{m.codex_ui_cancelled()}</Text> : null}
      {button(busy ? m.codex_ui_preparing() : m.contas_entrar(), () => void start(), busy || (!account && !name.trim()))}
      {accountId && account !== 'default' ? button(m.harness_codex_reconciliar(), () => void reconcile()) : null}
    </>}
    {error ? <Text accessibilityRole="alert" style={styles.error}>{error}</Text> : null}
  </View>;
}
const styles = StyleSheet.create(theme => ({
  root: { gap: theme.base.space[2] },
  text: { color: theme.tokens.text.primary },
  error: { color: theme.tokens.status.error },
  button: { minHeight: 44, justifyContent: 'center', paddingHorizontal: 12, borderWidth: 1, borderColor: theme.tokens.border.default, borderRadius: theme.base.radius.md },
  input: { minHeight: 44, color: theme.tokens.text.primary, borderWidth: 1, borderColor: theme.tokens.border.default, borderRadius: theme.base.radius.md, paddingHorizontal: 12 },
}));
