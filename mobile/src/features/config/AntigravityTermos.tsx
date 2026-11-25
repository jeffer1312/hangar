import { useEffect, useMemo, useRef, useState } from 'react';
import { ActivityIndicator, Alert, ScrollView, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import * as Linking from 'expo-linking';
import {
  answerAntigravityTermsForServer, cancelAntigravityTermsForServer, getAntigravityTermsForServer, questionParts,
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
  termos_expirou: m.mobile_antigravity_terms_error_expired,
};

type Live = { server: Server; attemptId: string };

/** A etapa que a folha desenha: a do servidor, ou `stuck` quando o Cancelar falhou com a janela ainda aberta. */
export type TermosStage = AntigravityTermsAttempt['estado'] | 'stuck';

const detail = (a: AntigravityTermsAttempt | null) => a?.estado === 'termos' ? a.texto : a?.estado === 'falhou' ? a.codigo : '';
// A leitura de 1 s quase sempre traz a mesma etapa: o objeto de antes fica e a folha não redesenha.
const same = (a: AntigravityTermsAttempt | null, b: AntigravityTermsAttempt) => a?.estado === b.estado && detail(a) === detail(b);

// O terminal escondido pode ter ficado vivo no servidor: a pessoa precisa saber.
const alertNotClosed = (now: Live, e: unknown) =>
  Alert.alert(m.mobile_antigravity_terms_title(), `${m.contas_login_servidor({ nome: now.server.label })}\n${fail(e)}`);

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
  // O DELETE falhou com a janela aberta: só o Cancelar fica, e ele manda o DELETE de novo.
  const [stuck, setStuck] = useState(false);
  const live = useRef<Live | null>(null);
  // A etapa na tela, para a leitura de 1 s comparar sem redesenhar.
  const shown = useRef<AntigravityTermsAttempt | null>(null);
  // O erro na tela é de uma leitura: a próxima leitura boa o tira. O de uma ação fica até a etapa mudar.
  const readFailed = useRef(false);
  const generation = useRef(0);
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const reader = useRef<AbortController | null>(null);
  const done = useRef(onDone);
  done.current = onDone;

  const stopReading = () => { clearTimeout(timer.current); reader.current?.abort(); reader.current = null; };
  const show = (next: AntigravityTermsAttempt | null) => { shown.current = next; setAttempt(next); };
  const actionError = (text: string) => { readFailed.current = false; setError(text); };

  function settle(next: AntigravityTermsAttempt) {
    // Na etapa de login o terminal segue aberto até o painel fechar (DELETE).
    if (next.estado === 'login') return;
    live.current = null;
    if (next.estado !== 'concluido') { haptic.error(); return; }
    toast.ok(m.mobile_antigravity_terms_done());
    void Promise.resolve().then(() => done.current()).catch((e: unknown) => actionError(fail(e)));
  }

  async function read(g: number) {
    const now = live.current;
    if (!now || g !== generation.current) return;
    stopReading();
    const controller = new AbortController();
    reader.current = controller;
    try {
      const next = await getAntigravityTermsForServer(now.server, ACCOUNT, now.attemptId, controller.signal);
      if (g !== generation.current || controller.signal.aborted) return;
      const changed = !same(shown.current, next);
      if (changed) show(next);
      if (changed || readFailed.current) { readFailed.current = false; setError(''); }
      if (polling(next)) timer.current = setTimeout(() => void read(g), POLL_MS);
      else settle(next);
    } catch (e) {
      if (g !== generation.current || controller.signal.aborted) return;
      // 404: a tentativa acabou ou foi trocada por outra; insistir não traz ela de volta.
      const status = (e as { status?: number } | null)?.status;
      if (status === 404) {
        live.current = null;
        show(null);
        actionError(m.mobile_antigravity_terms_attempt_lost());
        haptic.error();
      } else if (status !== undefined && status >= 400 && status < 500) {
        // Recusa (401, 403, 409) não muda insistindo. A tentativa fica em `live`: sair ainda a encerra.
        show(null);
        actionError(fail(e));
        haptic.error();
      } else {
        readFailed.current = true;
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
        const orphan = { server: s, attemptId: next.attempt_id };
        if (orphan.attemptId) void cancelAntigravityTermsForServer(s, ACCOUNT, orphan.attemptId).catch((e: unknown) => alertNotClosed(orphan, e));
        return;
      }
      if (!next.attempt_id) throw new Error(m.erro_desconhecido());
      live.current = { server: s, attemptId: next.attempt_id };
      show(next);
      void read(g);
    } catch (e) {
      if (g === generation.current) { actionError(fail(e)); haptic.error(); }
    } finally {
      if (g === generation.current) setBusy(null);
    }
  }

  async function answer(opcao: 1 | 2) {
    const now = live.current;
    if (!now || busy) return;
    haptic.selection();
    const g = generation.current;
    setBusy('answer'); actionError('');
    try {
      await answerAntigravityTermsForServer(now.server, ACCOUNT, now.attemptId, opcao);
      // Sem isto os botões voltariam até a próxima leitura, e o segundo toque levaria 409.
      if (g === generation.current) { show({ attempt_id: now.attemptId, estado: 'respondendo' }); void read(g); }
    } catch (e) {
      if (g === generation.current) { actionError(fail(e)); haptic.error(); }
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
        if (dismissed) {
          alertNotClosed(now, e);
          onClose();
          return;
        }
        // Sem reler: o servidor pode já ter esquecido a tentativa, e a leitura daria "tentativa perdida".
        live.current = now;
        setBusy(null);
        setStuck(true);
        actionError((e as { code?: string } | null)?.code === 'erro_termos_janela_aberta' ? m.mobile_antigravity_terms_error_window_open() : fail(e));
        haptic.error();
        return;
      }
    }
    onClose();
  }

  async function openLink(link: string) {
    try { await Linking.openURL(link); } catch (e) { actionError(fail(e)); }
  }

  useEffect(() => {
    void start();
    return () => {
      ++generation.current;
      stopReading();
      const now = live.current;
      live.current = null;
      if (now) {
        void cancelAntigravityTermsForServer(now.server, ACCOUNT, now.attemptId).catch((e: unknown) => alertNotClosed(now, e));
      }
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps -- uma vez por folha; o contas.tsx remonta por servidor
  }, []);

  const stage: TermosStage | null = stuck ? 'stuck' : busy === 'start' ? 'iniciando' : attempt?.estado ?? null;
  const failed = attempt?.estado === 'falhou' ? (FAILURES[attempt.codigo] ?? m.mobile_antigravity_terms_failed)() : '';

  return (
    <Sheet open sizes={['large']} scrollable dismissible={busy !== 'cancel'} onDismiss={() => void cancel(true)}>
      <ScrollView contentContainerStyle={styles.root}>
        <TermosPainel serverLabel={server.label} stage={stage} text={attempt?.estado === 'termos' ? attempt.texto : ''}
          sending={busy === 'answer'} failed={failed} error={error} cancelDisabled={busy === 'cancel'}
          terminal={!!failed || (!!error && (!live.current || !attempt) && busy !== 'start')}
          onAnswer={(opcao) => void answer(opcao)} onLink={(link) => void openLink(link)} onSignIn={onSignIn}
          onCancel={() => void cancel()} onOpenTerminal={onOpenTerminal} />
      </ScrollView>
    </Sheet>
  );
}

/** O conteúdo da folha dos Termos numa etapa; a animação da novidade desenha o mesmo, sem servidor. */
export function TermosPainel({ serverLabel, stage, text, sending, failed, error, cancelDisabled, terminal,
  onAnswer, onLink, onSignIn, onCancel, onOpenTerminal }: {
  serverLabel: string;
  stage: TermosStage | null;
  /** O texto do agy na etapa dos Termos. */
  text: string;
  /** A opção tocada ainda vai ao servidor. */
  sending: boolean;
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
  const terms = useMemo(() => stage === 'termos' ? questionParts(text) : null, [stage, text]);
  const answering = stage === 'respondendo' || (sending && stage !== 'stuck');
  return (
    <View style={styles.body}>
      <Text accessibilityRole="header" style={[styles.title, { color: c.text }]}>{m.mobile_antigravity_terms_title()}</Text>
      <Text style={[styles.text, { color: c.muted }]}>{m.contas_login_servidor({ nome: serverLabel })}</Text>

      {stage === 'iniciando' ? (
        <View style={styles.line}>
          <ActivityIndicator color={c.accent} />
          <Text style={[styles.text, { color: c.text }]} accessibilityLiveRegion="polite">{m.mobile_antigravity_terms_starting()}</Text>
        </View>
      ) : null}

      {terms ? (
        <>
          <Text style={[styles.text, { color: c.muted }]}>{m.mobile_antigravity_terms_intro()}</Text>
          <Text selectable style={[styles.terms, { color: c.text, borderColor: c.border, backgroundColor: c.inset }]}>
            {terms.map((part, i) => part.kind === 'link' ? (
              <Text key={i} accessibilityRole="link" onPress={() => onLink(part.text)}
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

      {stage === 'login' ? (
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
