import { useEffect, useRef, useState } from 'react';
import { ActivityIndicator, Alert, AppState, ScrollView, Text, TextInput, View } from 'react-native';
import { KeyboardAvoidingView } from 'react-native-keyboard-controller';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import * as Linking from 'expo-linking';
import * as Clipboard from 'expo-clipboard';
import {
  antigravityLoginLink, cancelAntigravityLoginForServer, getAntigravityLoginForServer,
  startAntigravityLoginForServer, submitAntigravityLoginCodeForServer,
  type AntigravityAccount, type AntigravityLoginAttempt, type Server,
} from '@hangar/core';
import { Sheet } from '../../ui/Sheet';
import { haptic } from '../../ui/haptics';
import { Pill } from './PageHeader';
import { SectionCard } from './SectionCard';
import { SettingsRow } from './SettingsRow';
import { useSettingsColors } from './colors';
import { antigravityAccountLabel } from '../accounts/antigravity';
import * as m from '../../paraglide/messages';

const POLL_MS = 1000;
const fail = (e: unknown) => e instanceof Error ? e.message : m.erro_desconhecido();
const running = (a: AntigravityLoginAttempt | null) => a?.estado === 'iniciando' || a?.estado === 'aguardando_codigo';

type Live = { server: Server; id: string; attemptId: string };

/**
 * Login da conta Google pelo próprio agy: o servidor mostra o link do Google e escreve no processo o
 * código que a pessoa colar. O Hangar nunca vê o token. Sem `accountId`, entra na padrão: com terminal o
 * agy ignora a pasta de outra conta, então não há adicional.
 */
export function AntigravityContaLogin({ server, accounts, accountId, onComplete, onClose }: {
  server: Server;
  accounts: AntigravityAccount[];
  accountId?: string;
  onComplete: () => void | Promise<void>;
  onClose: () => void;
}) {
  const c = useSettingsColors();
  const { theme } = useUnistyles();
  const accountToSign = accountId ?? 'default';
  const [attempt, setAttempt] = useState<AntigravityLoginAttempt | null>(null);
  const [code, setCode] = useState('');
  const [sent, setSent] = useState(false);
  const [copied, setCopied] = useState(false);
  const [busy, setBusy] = useState<'start' | 'code' | 'cancel' | null>(null);
  const [error, setError] = useState('');
  const live = useRef<Live | null>(null);
  const generation = useRef(0);
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const reader = useRef<AbortController | null>(null);
  const complete = useRef(onComplete);
  complete.current = onComplete;

  const stopReading = () => { clearTimeout(timer.current); reader.current?.abort(); reader.current = null; };

  function finish(next: AntigravityLoginAttempt) {
    live.current = null;
    if (next.estado !== 'concluido') { haptic.error(); return; }
    haptic.success();
    void Promise.resolve().then(() => complete.current()).catch((e: unknown) => setError(fail(e)));
  }

  async function read(g: number) {
    const now = live.current;
    if (!now || g !== generation.current) return;
    stopReading();
    const controller = new AbortController();
    reader.current = controller;
    try {
      const next = await getAntigravityLoginForServer(now.server, now.id, now.attemptId, controller.signal);
      if (g !== generation.current) return;
      setAttempt(next);
      setError('');
      if (running(next)) timer.current = setTimeout(() => void read(g), POLL_MS);
      else finish(next);
    } catch (e) {
      if (g !== generation.current || controller.signal.aborted) return;
      setError(fail(e));
      // 404: a tentativa acabou ou foi trocada por outra; insistir não traz ela de volta.
      if ((e as { status?: number } | null)?.status === 404) live.current = null;
      else timer.current = setTimeout(() => void read(g), POLL_MS * 3);
    } finally {
      if (reader.current === controller) reader.current = null;
    }
  }

  async function start() {
    if (busy) return;
    const s = { ...server };
    const g = ++generation.current;
    stopReading();
    live.current = null;
    setBusy('start'); setError(''); setAttempt(null); setCode(''); setSent(false); setCopied(false);
    try {
      const id = accountToSign;
      const next = await startAntigravityLoginForServer(s, id);
      if (g !== generation.current) {
        void cancelAntigravityLoginForServer(s, id, next.attempt_id).catch(() => {});
        return;
      }
      live.current = { server: s, id, attemptId: next.attempt_id };
      setAttempt(next);
      void read(g);
    } catch (e) {
      if (g === generation.current) { setError(fail(e)); haptic.error(); }
    } finally {
      if (g === generation.current) setBusy(null);
    }
  }

  async function submit() {
    const now = live.current;
    if (!now || busy || sent) return;
    const value = code.trim();
    if (!value) { setError(m.erro_codigo_vazio()); return; }
    if (value.length > 4096 || /[\r\n\0]/.test(value)) { setError(m.erro_codigo_invalido()); return; }
    const g = generation.current;
    setBusy('code'); setError('');
    try {
      await submitAntigravityLoginCodeForServer(now.server, now.id, now.attemptId, value);
      if (g !== generation.current) return;
      setSent(true);
      void read(g);
    } catch (e) {
      if (g === generation.current) { setError(fail(e)); haptic.error(); }
    } finally {
      if (g === generation.current) setBusy(null);
    }
  }

  async function cancel(dismissed = false) {
    if (busy === 'cancel') return;
    const now = live.current;
    ++generation.current;
    stopReading();
    live.current = null;
    if (now) {
      setBusy('cancel');
      try {
        await cancelAntigravityLoginForServer(now.server, now.id, now.attemptId);
      } catch (e) {
        // O processo de login pode ter ficado vivo no servidor: a pessoa precisa saber.
        if (dismissed) {
          Alert.alert(m.mobile_antigravity_login_title(), `${m.contas_login_servidor({ nome: now.server.label })}\n${fail(e)}`);
          onClose();
          return;
        }
        live.current = now;
        setBusy(null);
        setError(fail(e));
        // A tentativa segue viva no servidor: volta a ler o estado dela.
        void read(generation.current);
        return;
      }
    }
    onClose();
  }

  async function external(copy: boolean) {
    const link = antigravityLoginLink(attempt?.link);
    if (!link) { setError(m.mobile_accounts_claude_invalid_url()); return; }
    try {
      if (copy) {
        await Clipboard.setStringAsync(link);
        setCopied(true);
        haptic.success();
      } else await Linking.openURL(link);
    } catch (e) {
      setError(copy ? m.contas_login_copiar_erro() : fail(e));
    }
  }

  async function paste() {
    try {
      const text = await Clipboard.getStringAsync();
      if (text) setCode(text.trim());
    } catch (e) { setError(fail(e)); }
  }

  useEffect(() => {
    // Voltar do navegador do aparelho: confere na hora em vez de esperar o próximo tique.
    const listener = AppState.addEventListener('change', (state) => {
      if (state === 'active' && live.current) void read(generation.current);
    });
    void start();
    return () => {
      listener.remove();
      ++generation.current;
      stopReading();
      const now = live.current;
      live.current = null;
      if (now) {
        void cancelAntigravityLoginForServer(now.server, now.id, now.attemptId).catch((e: unknown) => {
          Alert.alert(m.mobile_antigravity_login_title(), `${m.contas_login_servidor({ nome: now.server.label })}\n${fail(e)}`);
        });
      }
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps -- uma vez por folha; o contas.tsx remonta por servidor
  }, []);

  const done = attempt?.estado === 'concluido';
  const failed = attempt?.estado === 'falhou';
  const waiting = attempt?.estado === 'aguardando_codigo';
  const tail = attempt?.saida?.filter((line) => line.trim()).at(-1);
  const known = accounts.find((a) => a.id === accountToSign);
  const title = known ? antigravityAccountLabel(known) : accountToSign === 'default' ? m.mobile_antigravity_default_account() : m.mobile_antigravity_login_title();

  return (
    <Sheet open sizes={['large']} scrollable dismissible={!busy} onDismiss={() => void cancel(true)}>
      <KeyboardAvoidingView automaticOffset behavior="padding" style={styles.fill}>
        <ScrollView keyboardShouldPersistTaps="handled" contentContainerStyle={styles.root}>
          <Text accessibilityRole="header" style={[styles.title, { color: c.text }]}>{title}</Text>
          <Text style={[styles.text, { color: c.muted }]}>{m.contas_login_servidor({ nome: server.label })}</Text>

          {busy === 'start' || attempt?.estado === 'iniciando' ? (
            <View style={styles.line}>
              <ActivityIndicator color={c.accent} />
              <Text style={[styles.text, { color: c.text }]} accessibilityLiveRegion="polite">{m.mobile_antigravity_starting()}</Text>
            </View>
          ) : null}

          {waiting ? (
            <SectionCard>
              <SettingsRow title={m.mobile_antigravity_step_link()}>
                {attempt?.link ? <Text selectable numberOfLines={4} style={[styles.link, { color: c.accentText, borderColor: c.border, backgroundColor: c.inset }]}>{attempt?.link}</Text> : null}
                <View style={styles.actions}>
                  <Pill icon="ExternalLink" label={m.contas_login_abrir()} disabled={!attempt?.link} onPress={() => void external(false)} />
                  <Pill icon="Copy" label={copied ? m.mobile_antigravity_copied() : m.contas_login_copiar_link()} disabled={!attempt?.link} onPress={() => void external(true)} />
                </View>
              </SettingsRow>
              <SettingsRow title={m.mobile_antigravity_step_code()}>
                <TextInput accessibilityLabel={m.contas_login_codigo()} placeholder={m.contas_codigo_placeholder()} value={code}
                  onChangeText={setCode} autoCapitalize="none" autoCorrect={false} spellCheck={false} editable={!busy && !sent}
                  returnKeyType="send" onSubmitEditing={() => void submit()} placeholderTextColor={c.faint}
                  style={[styles.input, { color: c.text, borderColor: c.borderStrong, backgroundColor: c.inset }]} />
                <View style={styles.actions}>
                  <Pill icon="ClipboardPaste" label={m.mobile_antigravity_paste()} disabled={!!busy || sent} onPress={() => void paste()} />
                  <Pill label={m.mobile_antigravity_send_code()} disabled={!!busy || sent || !code.trim()} onPress={() => void submit()} />
                </View>
              </SettingsRow>
            </SectionCard>
          ) : null}
          {waiting && (sent || busy === 'code') ? (
            <View style={styles.line}>
              <ActivityIndicator color={c.accent} />
              <Text style={[styles.text, { color: c.text }]} accessibilityLiveRegion="polite">{m.contas_login_confirmando()}</Text>
            </View>
          ) : null}

          {done ? (
            <SectionCard>
              <SettingsRow icon="CircleCheck" title={m.contas_login_conectada()} description={m.contas_login_pronta()} />
            </SectionCard>
          ) : null}
          {failed ? (
            <Text accessibilityRole="alert" style={[styles.text, { color: theme.tokens.status.error }]}>
              {[m.mobile_antigravity_login_failed(), tail].filter(Boolean).join('\n')}
            </Text>
          ) : null}

          {!done ? <Text style={[styles.note, { color: theme.tokens.status.warning }]}>{m.mobile_antigravity_terms_hint()}</Text> : null}
          {error ? <Text accessibilityRole="alert" style={[styles.text, { color: theme.tokens.status.error }]}>{error}</Text> : null}

          <View style={styles.actions}>
            {done ? <Pill label={m.contas_login_concluir()} onPress={onClose} />
              : <Pill label={m.comum_cancelar()} disabled={busy === 'cancel'} onPress={() => void cancel()} />}
            {failed || (error && !live.current && !done) ? <Pill label={m.novacred_codex_tentar()} disabled={!!busy} onPress={() => void start()} /> : null}
          </View>
        </ScrollView>
      </KeyboardAvoidingView>
    </Sheet>
  );
}

const styles = StyleSheet.create((theme) => ({
  fill: { flex: 1 },
  root: { padding: theme.base.space[4], paddingBottom: theme.base.space[8], gap: theme.base.space[3] },
  title: { fontSize: theme.base.text.lg, fontWeight: theme.base.weight.semibold },
  text: { fontSize: theme.base.text.sm },
  note: { fontSize: theme.base.text.xs, lineHeight: 18 },
  line: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2] },
  actions: { flexDirection: 'row', flexWrap: 'wrap', gap: theme.base.space[2] },
  input: { minHeight: 44, borderWidth: 1, borderRadius: theme.base.radius.md, paddingHorizontal: theme.base.space[3], fontSize: theme.base.text.sm },
  link: { fontFamily: theme.base.fontMono, fontSize: theme.base.text.xs, borderWidth: 1, borderRadius: theme.base.radius.md,
    padding: theme.base.space[2] },
}));
