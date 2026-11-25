import { useEffect, useRef, useState } from 'react';
import { ActivityIndicator, Alert, ScrollView, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import * as Linking from 'expo-linking';
import {
  answerAntigravityTermsForServer, cancelAntigravityTermsForServer, getAntigravityTermsForServer,
  startAntigravityTermsForServer, type AntigravityTermsAttempt, type Server,
} from '@hangar/core';
import { Sheet } from '../../ui/Sheet';
import { haptic } from '../../ui/haptics';
import { toast } from '../../ui/Toast';
import { Pill } from './PageHeader';
import { useSettingsColors } from './colors';
import * as m from '../../paraglide/messages';

const POLL_MS = 1000;
// Só a padrão: com terminal o agy ignora a pasta de outra conta.
const ACCOUNT = 'default';
const fail = (e: unknown) => e instanceof Error ? e.message : m.erro_desconhecido();
const polling = (a: AntigravityTermsAttempt | null) => a?.estado === 'iniciando' || a?.estado === 'termos' || a?.estado === 'respondendo';
const FAILURES: Record<string, () => string> = {
  termos_prazo: m.mobile_antigravity_terms_error_deadline,
  termos_tela_desconhecida: m.mobile_antigravity_terms_error_screen_gone,
  termos_sem_confirmacao: m.mobile_antigravity_terms_error_unconfirmed,
  termos_nao_convergiu: m.mobile_antigravity_terms_error_no_converge,
  termos_terminal_falhou: m.mobile_antigravity_terms_error_terminal,
};

/** Trechos do texto do agy, com os endereços https que dá para tocar. */
function parts(text: string): { text: string; link?: string }[] {
  const out: { text: string; link?: string }[] = [];
  let last = 0;
  for (const match of text.matchAll(/https:\/\/[^\s<>"]+/g)) {
    const raw = match[0].replace(/[.,;:!?)\]]+$/, '');
    let link: string | undefined;
    try { const url = new URL(raw); link = url.username || url.password ? undefined : url.href; } catch { link = undefined; }
    if (!link) continue;
    out.push({ text: text.slice(last, match.index) }, { text: raw, link });
    last = match.index + raw.length;
  }
  out.push({ text: text.slice(last) });
  return out.filter((part) => part.text);
}

type Live = { server: Server; attemptId: string };

/**
 * Termos do Google do agy pela página de contas: o servidor conduz o agy num terminal escondido e
 * publica a etapa; aqui só se desenha e se manda a opção que a pessoa tocou.
 */
export function AntigravityTermos({ server, onDone, onSignIn, onOpenTerminal, onClose }: {
  server: Server;
  onDone: () => void | Promise<void>;
  onSignIn: () => void;
  onOpenTerminal: () => void;
  onClose: () => void;
}) {
  const [attempt, setAttempt] = useState<AntigravityTermsAttempt | null>(null);
  const [busy, setBusy] = useState<'start' | 'answer' | 'cancel' | null>(null);
  const [error, setError] = useState('');
  const live = useRef<Live | null>(null);
  const generation = useRef(0);
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const reader = useRef<AbortController | null>(null);
  const done = useRef(onDone);
  done.current = onDone;

  const stopReading = () => { clearTimeout(timer.current); reader.current?.abort(); reader.current = null; };

  function settle(next: AntigravityTermsAttempt) {
    // Na etapa de login o terminal segue aberto até o painel fechar (DELETE).
    if (next.estado === 'login') return;
    live.current = null;
    if (next.estado !== 'concluido') { haptic.error(); return; }
    toast.ok(m.mobile_antigravity_terms_done());
    void Promise.resolve().then(() => done.current()).catch((e: unknown) => setError(fail(e)));
  }

  async function read(g: number) {
    const now = live.current;
    if (!now || g !== generation.current) return;
    stopReading();
    const controller = new AbortController();
    reader.current = controller;
    try {
      const next = await getAntigravityTermsForServer(now.server, ACCOUNT, now.attemptId, controller.signal);
      if (g !== generation.current) return;
      setAttempt(next);
      setError('');
      if (polling(next)) timer.current = setTimeout(() => void read(g), POLL_MS);
      else settle(next);
    } catch (e) {
      if (g !== generation.current || controller.signal.aborted) return;
      // 404: a tentativa acabou ou foi trocada por outra; insistir não traz ela de volta.
      if ((e as { status?: number } | null)?.status === 404) {
        live.current = null;
        setError(m.mobile_antigravity_terms_attempt_lost());
        haptic.error();
      } else {
        setError(fail(e));
        timer.current = setTimeout(() => void read(g), POLL_MS * 3);
      }
    } finally {
      if (reader.current === controller) reader.current = null;
    }
  }

  async function start() {
    const s = { ...server };
    const g = ++generation.current;
    setBusy('start');
    try {
      const next = await startAntigravityTermsForServer(s, ACCOUNT);
      if (g !== generation.current) {
        if (next.attempt_id) void cancelAntigravityTermsForServer(s, ACCOUNT, next.attempt_id).catch(() => {});
        return;
      }
      if (!next.attempt_id) throw new Error(m.erro_desconhecido());
      live.current = { server: s, attemptId: next.attempt_id };
      setAttempt(next);
      void read(g);
    } catch (e) {
      if (g === generation.current) { setError(fail(e)); haptic.error(); }
    } finally {
      if (g === generation.current) setBusy(null);
    }
  }

  async function answer(opcao: 1 | 2) {
    const now = live.current;
    if (!now || busy) return;
    haptic.selection();
    const g = generation.current;
    setBusy('answer'); setError('');
    try {
      await answerAntigravityTermsForServer(now.server, ACCOUNT, now.attemptId, opcao);
      if (g === generation.current) void read(g);
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
        await cancelAntigravityTermsForServer(now.server, ACCOUNT, now.attemptId);
      } catch (e) {
        // O terminal escondido pode ter ficado vivo no servidor: a pessoa precisa saber.
        if (dismissed) {
          Alert.alert(m.mobile_antigravity_terms_title(), `${m.contas_login_servidor({ nome: now.server.label })}\n${fail(e)}`);
          onClose();
          return;
        }
        live.current = now;
        setBusy(null);
        setError(fail(e));
        const g = generation.current;
        timer.current = setTimeout(() => void read(g), POLL_MS);
        return;
      }
    }
    onClose();
  }

  async function openLink(link: string) {
    try { await Linking.openURL(link); } catch (e) { setError(fail(e)); }
  }

  useEffect(() => {
    void start();
    return () => {
      ++generation.current;
      stopReading();
      const now = live.current;
      live.current = null;
      if (now) {
        void cancelAntigravityTermsForServer(now.server, ACCOUNT, now.attemptId).catch((e: unknown) => {
          Alert.alert(m.mobile_antigravity_terms_title(), `${m.contas_login_servidor({ nome: now.server.label })}\n${fail(e)}`);
        });
      }
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps -- uma vez por folha; o contas.tsx remonta por servidor
  }, []);

  const stage = attempt?.estado;
  const failed = attempt?.estado === 'falhou' ? (FAILURES[attempt.codigo] ?? m.mobile_antigravity_terms_failed)() : '';
  const answering = busy === 'answer' || stage === 'respondendo';

  return (
    <Sheet open sizes={['large']} scrollable dismissible={busy !== 'cancel'} onDismiss={() => void cancel(true)}>
      <ScrollView contentContainerStyle={styles.root}>
        <TermosPainel serverLabel={server.label} starting={busy === 'start' || stage === 'iniciando'}
          terms={attempt?.estado === 'termos' ? attempt.texto : null} answering={answering} login={stage === 'login'}
          failed={failed} error={error} cancelDisabled={busy === 'cancel'}
          terminal={!!failed || (!!error && !live.current && busy !== 'start')}
          onAnswer={(opcao) => void answer(opcao)} onLink={(link) => void openLink(link)} onSignIn={onSignIn}
          onCancel={() => void cancel()} onOpenTerminal={onOpenTerminal} />
      </ScrollView>
    </Sheet>
  );
}

/** O conteúdo da folha dos Termos numa etapa; a animação da novidade desenha o mesmo, sem servidor. */
export function TermosPainel({ serverLabel, starting, terms, answering, login, failed, error, cancelDisabled, terminal,
  onAnswer, onLink, onSignIn, onCancel, onOpenTerminal }: {
  serverLabel: string;
  starting: boolean;
  /** O texto do agy quando a etapa é a dos Termos. */
  terms: string | null;
  answering: boolean;
  login: boolean;
  failed: string;
  error: string;
  cancelDisabled: boolean;
  /** Oferece "Abrir no terminal". */
  terminal: boolean;
  onAnswer: (opcao: 1 | 2) => void;
  onLink: (link: string) => void;
  onSignIn: () => void;
  onCancel: () => void;
  onOpenTerminal: () => void;
}) {
  const c = useSettingsColors();
  const { theme } = useUnistyles();
  return (
    <View style={styles.body}>
      <Text accessibilityRole="header" style={[styles.title, { color: c.text }]}>{m.mobile_antigravity_terms_title()}</Text>
      <Text style={[styles.text, { color: c.muted }]}>{m.contas_login_servidor({ nome: serverLabel })}</Text>

      {starting ? (
        <View style={styles.line}>
          <ActivityIndicator color={c.accent} />
          <Text style={[styles.text, { color: c.text }]} accessibilityLiveRegion="polite">{m.mobile_antigravity_terms_starting()}</Text>
        </View>
      ) : null}

      {terms !== null ? (
        <>
          <Text style={[styles.text, { color: c.muted }]}>{m.mobile_antigravity_terms_intro()}</Text>
          <Text selectable style={[styles.terms, { color: c.text, borderColor: c.border, backgroundColor: c.inset }]}>
            {parts(terms).map((part, i) => part.link ? (
              <Text key={i} accessibilityRole="link" onPress={() => onLink(part.link!)}
                style={[styles.link, { color: c.accentText }]}>{part.text}</Text>
            ) : part.text)}
          </Text>
          {!answering ? (
            <View style={styles.actions}>
              <Pill icon="Check" label={m.mobile_antigravity_terms_accept_share()} onPress={() => onAnswer(1)} />
              <Pill icon="Check" label={m.mobile_antigravity_terms_accept_private()} onPress={() => onAnswer(2)} />
            </View>
          ) : null}
        </>
      ) : null}

      {answering ? (
        <View style={styles.line}>
          <ActivityIndicator color={c.accent} />
          <Text style={[styles.text, { color: c.text }]} accessibilityLiveRegion="polite">{m.mobile_antigravity_terms_answering()}</Text>
        </View>
      ) : null}

      {login ? (
        <>
          <Text style={[styles.text, { color: c.text }]}>{m.mobile_antigravity_terms_login()}</Text>
          <View style={styles.actions}><Pill icon="LogIn" label={m.contas_entrar()} onPress={onSignIn} /></View>
        </>
      ) : null}

      {failed ? <Text accessibilityRole="alert" style={[styles.text, { color: theme.tokens.status.error }]}>{failed}</Text> : null}
      {error ? <Text accessibilityRole="alert" style={[styles.text, { color: theme.tokens.status.error }]}>{error}</Text> : null}

      <View style={styles.actions}>
        <Pill label={m.mobile_antigravity_terms_cancel()} disabled={cancelDisabled} onPress={onCancel} />
        {terminal ? <Pill icon="Terminal" label={m.mobile_antigravity_terms_open_terminal()} onPress={onOpenTerminal} /> : null}
      </View>
    </View>
  );
}

const styles = StyleSheet.create((theme) => ({
  root: { padding: theme.base.space[4], paddingBottom: theme.base.space[8] },
  body: { gap: theme.base.space[3] },
  title: { fontSize: theme.base.text.lg, fontWeight: theme.base.weight.semibold },
  text: { fontSize: theme.base.text.sm },
  terms: { fontSize: theme.base.text.sm, lineHeight: 20, borderWidth: 1, borderRadius: theme.base.radius.md, padding: theme.base.space[3] },
  link: { textDecorationLine: 'underline' },
  line: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2] },
  actions: { flexDirection: 'row', flexWrap: 'wrap', gap: theme.base.space[2] },
}));
