import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { AccessibilityInfo, ActivityIndicator, AppState, Pressable, ScrollView, Text, TextInput, View } from 'react-native';
import Animated, {
  Easing, useAnimatedStyle, useReducedMotion, useSharedValue, withDelay, withSpring, withTiming,
} from 'react-native-reanimated';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { useRouter } from 'expo-router';
import { KeyboardAvoidingView } from 'react-native-keyboard-controller';
import { SettingsList } from '../../ui/SettingsList';
import { ComposerActions, ComposerBox, ComposerDock, ComposerInput, ComposerViewport } from '../../chat/ComposerField';
import { formataErro, replaceSlashToken, slashMatches, slashTokenAt, type CommandInfo, type PrewarmTicket, type Server } from '@hangar/core';
import { useNewChatCommands, type NewChatCommandSource } from '../../chat/newChatCommands';
import { SlashSuggest } from '../../chat/SlashSuggest';
import { AnchoredPanel } from '../../ui/AnchoredPanel';
import { Icon } from '../../ui/Icon';
import { Glass } from '../../ui/Glass';
import type { HomeScreen } from '../../stores/aparencia';
import { AttachmentPreview } from '../../ui/AttachmentPreview';
import { fromPastedFile, readClipboard, type PickedAttachment } from '../../ui/attachmentPicker';
import { AttachSheet } from '../../ui/AttachSheet';
import { usePressScale } from '../../ui/pressScale';
import { DictationStyleMenu, useDictationStyleLabel } from '../ditado/EstiloPill';
import { removeDraftAttachment, retainDraftAttachment } from '../../chat/draftAttachments';
import { withoutUpload } from '../../stores/drafts';
import { superficie } from '../../theme/superficie';
import {
  adoptCandidate, attemptAttachment, beginAttempt, discardAttempt, recoverAttempt, restoreAttempt, sendFirstInput,
  useNewConversation, type NewConversationInput,
} from '../../stores/newConversation';
import { HomeIntroMark, HomeQuietMark } from './HomeAmbient';
import { useHomeDictation } from './useHomeDictation';
import * as m from '../../paraglide/messages';
import { useTourAnchor } from '../tour/TourAnchor';
import { reportTourAction } from '../tour/anchors';

const AnimatedPressable = Animated.createAnimatedComponent(Pressable);
const RISE_SPRING = { damping: 18, stiffness: 210, mass: 1 };

export type HomePresentation = {
  mode: HomeScreen;
  focusRequest: number;
  onNewConversation: (focus: boolean) => void;
  sessions: ReactNode;
};

type Props = {
  home?: HomePresentation;
  server: Server;
  destination: ReactNode;
  destinationPending: boolean;
  providerLabel: string;
  // Painel de pastas da pílula; `close` fecha o painel depois da escolha final.
  folderPanel: (close: () => void) => ReactNode;
  // Título acima da caixa, com a pasta tocável ("O que vamos fazer em [pasta]?").
  topPills: (openFolder: (opener: View | null) => void) => ReactNode;
  // Onde o agente trabalha, conta e terminal, numa linha acima da caixa.
  optionPills: ReactNode;
  providerChip: ReactNode;
  // Conversas antigas desta pasta que dá para continuar (hoje só o Codex lista).
  resume: { entries: { id: string; label: string }[]; busy: boolean; onPick: (id: string) => void } | null;
  notices: ReactNode;
  // Logo acima da linha de pílulas (a conta que a cota tirou, com o "abrir mesmo assim").
  optionNotice?: ReactNode;
  // null enquanto falta destino ou conta; o texto continua editável.
  body: NewConversationInput['body'] | null;
  // Reserva pré-aquecida do corpo atual; chamar para a renovação dela antes do POST.
  takePrewarm?: () => PrewarmTicket | null;
  // A criação falhou e a tela continua: volta a reservar.
  resumePrewarm?: () => void;
  blocked: boolean;
  // De onde vem a lista do `/` (conta e pasta escolhidas); null quando o agente não tem a lista sem sessão.
  commandSource?: NewChatCommandSource | null;
  // Distância fixa do topo da janela até a tela; sem ela o recuo do teclado é medido na janela.
  keyboardOffset?: number;
  // "Abrir na <máquina>" da regra de projeto: `server` encaminha o pedido para `at`, que a máquina informa como `machine`;
  // a tentativa, a primeira mensagem e a conversa ficam em `at`.
  forward?: { at: Server; machine: string } | null;
};

// O form remonta ao trocar de máquina: o que já foi digitado e anexado vai junto.
const carried: { text: string; attachment: PickedAttachment | null } = { text: '', attachment: null };
export function _resetCarriedForTests(): void {
  carried.text = '';
  carried.attachment = null;
}

// Sem a cópia do app sobra só um arquivo órfão; nada que a pessoa escreveu se perde.
function dropCopy(uri: string): void {
  try { removeDraftAttachment(uri); } catch { /* sem perda de dado */ }
}

function HistoryButton({ onPress, disabled, busy }: { onPress: () => void; disabled: boolean; busy: boolean }) {
  const { theme } = useUnistyles();
  const press = usePressScale();
  return (
    <AnimatedPressable
      onPress={onPress}
      onPressIn={press.onPressIn}
      onPressOut={press.onPressOut}
      disabled={disabled}
      hitSlop={5}
      accessibilityRole="button"
      accessibilityLabel={m.criar_retomar()}
      accessibilityState={{ disabled: !!disabled, busy: !!busy }}
      style={[styles.tool, press.style, disabled && styles.dim]}
    >
      {busy
        ? <ActivityIndicator size="small" color={theme.tokens.text.secondary} />
        : <Icon name="History" size={20} color={theme.tokens.text.secondary} />}
    </AnimatedPressable>
  );
}

export function NewConversation({
  server, destination, destinationPending, providerLabel, folderPanel,
  topPills, optionPills, providerChip, resume, notices, optionNotice, body, takePrewarm, resumePrewarm, blocked, keyboardOffset, home,
  commandSource = null, forward = null,
}: Props) {
  const collapsed = home?.mode === 'sessions';
  const launcherAnchor = useTourAnchor(home && collapsed ? 'home-new' : null);
  const input = useRef<TextInput>(null);
  const focusedRequest = useRef(0);
  const router = useRouter();
  const { theme } = useUnistyles();
  const serverId = (forward?.at ?? server).id;
  const attempt = useNewConversation((s) => s.attempts[serverId] ?? null);
  const issue = useNewConversation((s) => s.issues[serverId] ?? null);
  const busy = useNewConversation((s) => !!s.busy[serverId]);
  const [text, setText] = useState(carried.text);
  const [attachment, setAttachment] = useState<PickedAttachment | null>(carried.attachment);
  const [attachError, setAttachError] = useState('');
  const [attachMenuOpen, setAttachMenuOpen] = useState(false);
  const [styleMenuOpen, setStyleMenuOpen] = useState(false);
  const [focused, setFocused] = useState(false);
  const dictationStyle = useDictationStyleLabel(server);
  const [sheet, setSheet] = useState<'folder' | 'resume' | null>(null);
  const resumeAnchor = useRef<View>(null);
  const sheetOpener = useRef<View | null>(null);
  // O painel de pasta sobe da linha acima da caixa, não do título no meio da tela: dali cabem várias pastas.
  const folderAnchor = useRef<View>(null);
  // Enviar só depois de ler a tentativa gravada: antes disso um toque poderia seguir a antiga.
  const [restored, setRestored] = useState(false);
  const mounted = useRef(true);
  const submitting = useRef(false);

  // Abertura coreografada: roda ao montar e quando o app volta do segundo plano. Voltar de uma
  // conversa não reanima: a tela já aparece inteira durante o gesto, e zerar no fim dele pulava.
  // Só mexe em opacidade e posição: o toque vale desde o início.
  const reduced = useReducedMotion();
  const [intro, setIntro] = useState(() => ({ epoch: 1, at: Date.now() }));
  useEffect(() => {
    let last = AppState.currentState;
    const sub = AppState.addEventListener('change', (next) => {
      if (last === 'background' && next === 'active') setIntro((i) => ({ epoch: i.epoch + 1, at: Date.now() }));
      last = next;
    });
    return () => sub.remove();
  }, []);
  const rise = useSharedValue(reduced ? 1 : 0);
  const riseTop = useSharedValue(reduced ? 1 : 0);
  useEffect(() => {
    if (reduced) { rise.value = 1; riseTop.value = 1; return; }
    rise.value = 0;
    riseTop.value = 0;
    rise.value = withSpring(1, RISE_SPRING);
    riseTop.value = withDelay(80, withSpring(1, RISE_SPRING));
  }, [intro.epoch, reduced, rise, riseTop]);
  const riseStyle = useAnimatedStyle(() => ({
    opacity: Math.min(1, rise.value * 1.4),
    transform: [{ translateY: (1 - rise.value) * 28 }],
  }));
  const riseTopStyle = useAnimatedStyle(() => ({
    opacity: Math.min(1, riseTop.value * 1.4),
    transform: [{ translateY: (1 - riseTop.value) * 20 }],
  }));

  useEffect(() => { carried.text = text; }, [text]);
  useEffect(() => { carried.attachment = attachment; }, [attachment]);

  const dictation = useHomeDictation(server, (spoken) => {
    setText((current) => (current.trim() ? `${current.trimEnd()} ${spoken}` : spoken));
  });

  const expansion = useSharedValue(collapsed ? 0 : 1);
  const formHeight = useSharedValue(220);
  const launcherHeight = useSharedValue(56);
  useEffect(() => {
    const target = collapsed ? 0 : 1;
    expansion.value = reduced ? target : withTiming(target, { duration: 320, easing: Easing.out(Easing.cubic) });
  }, [collapsed, reduced, expansion]);
  const listStyle = useAnimatedStyle(() => ({ opacity: 1 - expansion.value, transform: [{ translateY: -60 * expansion.value }] }));
  const conversationStyle = useAnimatedStyle(() => ({ opacity: expansion.value, transform: [{ translateY: 40 * (1 - expansion.value) }] }));
  const frameStyle = useAnimatedStyle(() => ({ height: launcherHeight.value + (formHeight.value - launcherHeight.value) * expansion.value }));
  const formStyle = useAnimatedStyle(() => ({ opacity: expansion.value, transform: [{ translateY: 16 * (1 - expansion.value) }] }));
  const launcherStyle = useAnimatedStyle(() => ({ opacity: 1 - expansion.value }));

  useEffect(() => {
    if (!home) return;
    if (collapsed) { focusedRequest.current = home.focusRequest; input.current?.blur(); return; }
    if (home.focusRequest > focusedRequest.current && !dictation.gravando && !dictation.transcribing) {
      focusedRequest.current = home.focusRequest;
      input.current?.focus();
    }
  }, [home?.focusRequest, collapsed, dictation.gravando, dictation.transcribing]);
  useEffect(() => {
    // Voltar para a lista finaliza e transcreve; o microfone nunca continua escondido.
    if (collapsed && dictation.gravando) void dictation.stop();
  }, [collapsed, dictation.gravando, dictation.stop]);

  // A caixa sempre cabe acima do teclado: o teto do campo é o que sobra da área depois das pílulas,
  // do anexo, dos botões e de uma faixa do meio que segue arrastável para baixar o teclado.
  const [areaHeight, setAreaHeight] = useState(0);
  const [blockHeight, setBlockHeight] = useState(0);
  const [fieldHeight, setFieldHeight] = useState(0);
  // Tela baixa (iPhone SE, celular deitado): a faixa do meio cede antes do campo, senão o Enviar sai da área.
  const middleReserve = Math.min(MIDDLE_RESERVE, Math.max(MIDDLE_MIN_RESERVE, areaHeight * 0.15));
  const fieldMax = areaHeight && blockHeight && fieldHeight
    ? Math.max(FIELD_MIN_HEIGHT, Math.min(FIELD_MAX_HEIGHT, areaHeight - middleReserve - (blockHeight - fieldHeight)))
    : FIELD_MAX_HEIGHT;
  // A marca só entra se couber inteira no vazio acima do título.
  const [markRoom, setMarkRoom] = useState(0);

  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; };
  }, []);

  useEffect(() => {
    const saved = restoreAttempt(serverId);
    // Texto com ACK não volta ao campo: descartar e enviar criaria outra conversa com ele.
    if (saved && saved.phase !== 'sent') {
      setText((current) => current || saved.text);
      const kept = attemptAttachment(saved);
      if (kept) setAttachment((current) => current ?? kept);
    }
    setRestored(true);
  }, [serverId]);

  // A cópia do anexo é da tentativa enquanto ela existir; só a de ninguém pode ser apagada.
  const heldByAttempt = (uri: string) => {
    const current = useNewConversation.getState().attempts[serverId];
    return !!current && attemptAttachment(current)?.uri === uri;
  };

  // Na tela inicial (base da pilha) a conversa empilha, e voltar chega aqui com o campo limpo;
  // na rota /create, empilhada sobre a inicial, ela toma o lugar da criação.
  const open = (name: string) => {
    const href = (`/s/${serverId}/${name}` as never) as never;
    carried.text = '';
    carried.attachment = null;
    if (router.canGoBack?.() ?? true) return router.replace(href);
    setText('');
    setAttachment(null);
    router.push(href);
  };

  // Cria e envia em passos separados; o chat abre mesmo com input recusado ou incerto.
  const advance = async () => {
    if (submitting.current) return;
    submitting.current = true;
    try { await advanceOnce(); } finally { submitting.current = false; }
  };

  const advanceOnce = async () => {
    let current = useNewConversation.getState().attempts[serverId];
    if (current?.phase === 'created') {
      await sendFirstInput(serverId, current.id);
      current = useNewConversation.getState().attempts[serverId];
    }
    if (!mounted.current || !current?.sessionName) return;
    // Entregue, a cópia local do anexo já não serve a ninguém.
    const sent = current.phase === 'sent' ? attemptAttachment(current) : null;
    if (sent) dropCopy(sent.uri);
    if (current.phase === 'created' || current.phase === 'sent' || current.phase === 'send_unknown') open(current.sessionName);
  };

  const recording = dictation.gravando || dictation.transcribing;

  // ── Comandos: as sugestões do `/` da conversa aberta, com a lista da conta e pasta escolhidas ──
  // Cursor movido pelo app (`selection`) vale até o campo devolver o dele, como no Composer.
  const [selection, setSelection] = useState<{ start: number; end: number } | undefined>();
  const [caret, setCaret] = useState<{ start: number; end: number } | null>(null);
  const sel = selection ?? caret;
  const caretPos = !sel ? text.length : sel.start === sel.end ? Math.min(sel.end, text.length) : -1;
  const slashToken = useMemo(() => slashTokenAt(text, caretPos), [text, caretPos]);
  const slashQuery = commandSource ? slashToken?.query ?? null : null;
  const { commands, error: commandsError, retry: retryCommands } = useNewChatCommands(server, commandSource, slashQuery !== null);
  const suggestions = useMemo(() => slashMatches(commands ?? [], slashQuery), [commands, slashQuery]);
  // Sem sessão ainda não há para onde enviar: a escolha só vira o texto do campo, e o Enviar cria a conversa com ele.
  const pickSlash = (c: CommandInfo) => {
    if (!slashToken) return;
    const next = replaceSlashToken(text, slashToken, c.name);
    setText(next.text);
    setSelection({ start: next.cursor, end: next.cursor });
  };

  const handleSend = async () => {
    if (!body || blocked || busy || !restored || recording || (!text.trim() && !attachment) || submitting.current) return;
    submitting.current = true;
    try {
      // A reserva é desta máquina: encaminhada, a sessão nasce fria lá.
      const ticket = forward ? null : takePrewarm?.() ?? null;
      await beginAttempt(serverId, {
        body: ticket ? { ...body, prewarm: ticket.id } : body, reserveName: ticket?.name,
        text, attachment: attachment ? withoutUpload(attachment) : null,
        forward: forward ? { via: server.id, machine: forward.machine } : null,
      });
      if (!useNewConversation.getState().attempts[serverId]?.sessionName) resumePrewarm?.();
      await advanceOnce();
    } finally { submitting.current = false; }
  };

  const adopt = async (picked: PickedAttachment) => {
    let kept: PickedAttachment;
    try {
      kept = await retainDraftAttachment(withoutUpload(picked));
    } catch (e) {
      setAttachError(e instanceof Error && e.message ? e.message : m.board_falha_upload());
      return;
    }
    if (!mounted.current) return;
    if (attachment && attachment.uri !== kept.uri && !heldByAttempt(attachment.uri)) dropCopy(attachment.uri);
    setAttachment({ ...kept, size: picked.size });
  };

  const pick = (picked: PickedAttachment) => {
    setAttachError('');
    void adopt(picked);
  };

  const removeAttachment = () => {
    if (!attachment) return;
    if (!heldByAttempt(attachment.uri)) dropCopy(attachment.uri);
    setAttachment(null);
  };

  const discard = (id: string) => {
    const held = pending ? attemptAttachment(pending) : null;
    discardAttempt(serverId, id);
    // Sessão já criada guarda o anexo no rascunho dela; sem sessão e fora do campo, ninguém mais o usa.
    if (held && !pending?.sessionName && held.uri !== attachment?.uri && !useNewConversation.getState().attempts[serverId]) dropCopy(held.uri);
  };

  const openSheet = (kind: 'folder' | 'resume', opener: View | null) => {
    sheetOpener.current = opener;
    setSheet(kind);
  };
  const closeSheet = useCallback(() => setSheet(null), []);
  const onSheetDismiss = () => {
    closeSheet();
    if (mounted.current && sheetOpener.current) AccessibilityInfo.sendAccessibilityEvent(sheetOpener.current, 'focus');
  };

  const pending = attempt && attempt.phase !== 'draft' ? attempt : null;
  const receivedMessages = issue?.kind === 'unknown' && issue.events
    ? issue.events.filter((event) => event.kind === 'user_msg').slice(-3) : null;
  const hasContent = !!text.trim() || !!attachment;
  const canSend = !!body && !blocked && !busy && restored && !pending && !recording && hasContent;

  // Sem pasta escolhida o seletor ocupa o meio; escolhida, o meio fica com o título, como no PC.
  const showPicker = destinationPending && sheet === null;
  // Sem pasta o título fica colado à caixa, com o seletor no meio; pela pasta pendente, e não pelo
  // seletor à vista, para a pílula não mudar de lugar com o painel de pasta aberto preso a ela.
  const heroInMiddle = !destinationPending;
  // O teclado abre junto com a tela: o título fica à vista com ele, e o meio rola quando não cabe.
  const hero = (
    <Animated.View style={[styles.topPills, !home && riseTopStyle]}>{topPills((opener) => openSheet('folder', opener))}</Animated.View>
  );

  return (
    <View style={styles.root}>
      <KeyboardAvoidingView behavior="padding" automaticOffset={keyboardOffset === undefined}
                            keyboardVerticalOffset={keyboardOffset} style={styles.keyboard}>
      <View style={styles.area} onLayout={(event) => setAreaHeight(event.nativeEvent.layout.height)}>
      <ComposerViewport>
      <View style={styles.middle}>
      {home ? <Animated.View style={[styles.homeList, listStyle]} pointerEvents={collapsed ? 'auto' : 'none'}
        accessibilityElementsHidden={!collapsed} importantForAccessibility={collapsed ? 'auto' : 'no-hide-descendants'}>
        {home.sessions}
      </Animated.View> : null}
      <Animated.View style={[styles.states, home && conversationStyle]} pointerEvents={collapsed ? 'none' : 'auto'}
        accessibilityElementsHidden={collapsed} importantForAccessibility={collapsed ? 'no-hide-descendants' : 'auto'}>
      {/* Arrastar o meio para baixo fecha o teclado, como na conversa. */}
      <ScrollView style={styles.states} contentContainerStyle={styles.scroll} keyboardShouldPersistTaps="handled" keyboardDismissMode="on-drag">
        {!home && !reduced && !showPicker && !pending ? <HomeIntroMark epoch={intro.epoch} /> : null}
        {showPicker ? destination : null}
        {heroInMiddle ? (
          // Os dois vãos iguais centram o título; a marca mora no de cima e só aparece se couber.
          <View style={styles.heroArea}>
            <View style={styles.heroSpace} onLayout={(event) => setMarkRoom(event.nativeEvent.layout.height)}>
              {markRoom >= MARK_SIZE + theme.base.space[4] ? <HomeQuietMark size={MARK_SIZE} /> : null}
            </View>
            {hero}
            <View style={styles.heroSpace} />
          </View>
        ) : null}
        {notices}

        {pending ? (
          <View style={styles.pending}>
            <Text style={styles.label}>{m.nova_conversa_guardada()}</Text>
            <Text style={styles.pendingText} numberOfLines={3}>{pending.text}</Text>
            {attemptAttachment(pending) ? (
              <View style={styles.pendingAttach}>
                <Icon name="Paperclip" size={14} color={theme.tokens.text.muted} />
                <Text style={[styles.hint, styles.flex]} numberOfLines={1}>{attemptAttachment(pending)!.name}</Text>
              </View>
            ) : null}
            {receivedMessages ? (
              <View>
                <Text style={styles.label}>{m.new_conversation_received_messages()}</Text>
                {receivedMessages.length ? receivedMessages.map((event) => (
                  <Text key={event.id} style={styles.pendingText} numberOfLines={3}>{event.text ?? ''}</Text>
                )) : <Text style={styles.hint}>{m.new_conversation_no_received_messages()}</Text>}
              </View>
            ) : null}
            {pending.sessionName && (pending.phase === 'created' || pending.phase === 'sent' || pending.phase === 'send_unknown') ? (
              <Pressable accessibilityRole="button" accessibilityState={{ disabled: busy, busy }} onPress={() => open(pending.sessionName!)} disabled={busy} style={[styles.secondary, busy && styles.disabled]}>
                <Text style={styles.secondaryTxt}>{m.nova_conversa_abrir()}</Text>
              </Pressable>
            ) : null}
            {pending.phase === 'created' ? (
              <Pressable accessibilityRole="button" accessibilityState={{ disabled: busy, busy }} onPress={() => void advance()} disabled={busy} style={[styles.secondary, busy && styles.disabled]}>
                <Text style={styles.secondaryTxt}>{m.nova_conversa_reenviar()}</Text>
              </Pressable>
            ) : null}
            {pending.phase === 'create_unknown' || pending.phase === 'send_unknown' ? (
              <Pressable accessibilityRole="button" accessibilityState={{ disabled: busy, busy }} onPress={() => void recoverAttempt(serverId)} disabled={busy} style={[styles.secondary, busy && styles.disabled]}>
                <Text style={styles.secondaryTxt}>{m.nova_conversa_conferir()}</Text>
              </Pressable>
            ) : null}
            {issue?.kind === 'candidate' ? (
              <>
                <Pressable accessibilityRole="button" onPress={() => open(issue.session.name)} style={styles.secondary}>
                  <Text style={styles.secondaryTxt}>{m.nova_conversa_abrir()}</Text>
                </Pressable>
                <Pressable accessibilityRole="button" onPress={() => adoptCandidate(serverId, pending.id)} style={styles.secondary}>
                  <Text style={styles.secondaryTxt}>{m.nova_conversa_adotar()}</Text>
                </Pressable>
              </>
            ) : null}
            <Pressable accessibilityRole="button" accessibilityState={{ disabled: busy }} onPress={() => discard(pending.id)} disabled={busy} style={[styles.ghost, busy && styles.disabled]}>
              <Text style={styles.ghostTxt}>{m.nova_conversa_descartar()}</Text>
            </Pressable>
          </View>
        ) : null}

        {issue ? <Text style={styles.error} accessibilityRole="alert">{issue.message}</Text> : null}
        {!body && !blocked && !pending ? <Text style={styles.hint}>{m.nova_conversa_sem_destino()}</Text> : null}
      </ScrollView>
      </Animated.View>
      </View>

      <Animated.View style={home ? [styles.formFrame, frameStyle] : styles.shrink}>
      {home ? <Animated.View style={[styles.launcherPosition, launcherStyle]} pointerEvents={collapsed ? 'auto' : 'none'}
        accessibilityElementsHidden={!collapsed} importantForAccessibility={collapsed ? 'auto' : 'no-hide-descendants'}
        onLayout={(event) => { launcherHeight.value = event.nativeEvent.layout.height; }}>
        <Glass variant="chrome" style={styles.launcher}>
          <View style={styles.launcherRow}>
            <Pressable ref={launcherAnchor.ref} onLayout={launcherAnchor.onLayout} collapsable={false} style={styles.launcherText}
              accessibilityRole="button" accessibilityLabel={m.home_new_conversation()}
              onPress={() => { reportTourAction('new'); home.onNewConversation(true); }}>
              <Icon name="SquarePen" size={18} color={theme.tokens.text.secondary} />
              <Text style={styles.hint} numberOfLines={1}>{m.home_new_conversation()}</Text>
            </Pressable>
            <Pressable style={styles.launcherMic} accessibilityRole="button"
              accessibilityLabel={dictation.gravando ? m.composer_parar_gravacao() : m.composer_gravar_audio()}
              accessibilityState={{ disabled: dictation.transcribing || busy, busy: dictation.transcribing }}
              disabled={dictation.transcribing || busy}
              onPress={() => { home.onNewConversation(false); void dictation.toggle(); }}>
              {dictation.transcribing ? <ActivityIndicator size="small" />
                : <Icon name={dictation.gravando ? 'Square' : 'Mic'} size={20} color={theme.tokens.text.secondary} />}
            </Pressable>
          </View>
          {dictation.transcribing ? <Text style={styles.meta} accessibilityLiveRegion="polite">{m.composer_transcrevendo_audio()}</Text> : null}
          {dictation.error ? <Text style={styles.error} accessibilityRole="alert">{dictation.error}</Text> : null}
        </Glass>
      </Animated.View> : null}
      <Animated.View style={home ? [styles.measuredForm, formStyle] : styles.shrink} pointerEvents={collapsed ? 'none' : 'auto'}
        accessibilityElementsHidden={collapsed} importantForAccessibility={collapsed ? 'no-hide-descendants' : 'auto'}
        onLayout={home ? (event) => { formHeight.value = event.nativeEvent.layout.height; } : undefined}>
      {/* O teto da doca segura a caixa enquanto a medida do campo não chega: ela encolhe e o Enviar fica à vista. */}
      <ComposerDock onLayout={(event) => setBlockHeight(event.nativeEvent.layout.height)}>
      {!heroInMiddle ? (
        <ScrollView style={styles.dockHero} bounces={false} overScrollMode="never" keyboardShouldPersistTaps="handled" keyboardDismissMode="on-drag">{hero}</ScrollView>
      ) : null}

      <Animated.View style={[styles.shrink, !home && riseStyle]}>

      {optionNotice}
      <View ref={folderAnchor} collapsable={false} style={styles.optionPills}>{optionPills}</View>
      {/* Campo em cima, na largura toda; anexo, microfone, chip do provider e Enviar na linha de baixo. */}
      <ComposerBox focused={focused} footer={(
        <ComposerActions attach={{ onPress: () => setAttachMenuOpen(true), disabled: busy || dictation.gravando }}
          mic={{ onPress: () => void dictation.toggle(), disabled: dictation.transcribing || busy, busy: dictation.transcribing, showBusy: true,
            recording: dictation.gravando, styleLabel: dictationStyle, onStyle: () => setStyleMenuOpen(true) }}
          settings={providerChip}
          beforeMic={resume ? <View ref={resumeAnchor}><HistoryButton busy={resume.busy}
            disabled={busy || resume.busy} onPress={() => openSheet('resume', resumeAnchor.current)} /></View> : null}
          send={{ label: m.nova_conversa_enviar(), disabled: !canSend, busy, showBusy: true, onPress: () => void handleSend() }} />
      )}>
        {attachment ? <AttachmentPreview attachment={attachment} onRemove={removeAttachment} disabled={busy} /> : null}
        <SlashSuggest matches={suggestions} onPick={pickSlash}
          error={slashQuery !== null ? commandsError : ''} onRetry={retryCommands} />
        {/* Até cinco linhas e rola; com o teclado aberto, só o que cabe acima dele. */}
        <View onLayout={(event) => setFieldHeight(event.nativeEvent.layout.height)}>
        <ComposerInput ref={input} value={text} onChangeText={setText} onFocus={() => setFocused(true)} onBlur={() => setFocused(false)}
          selection={selection}
          onSelectionChange={(e) => { setSelection(undefined); setCaret(e.nativeEvent.selection); }}
          maxHeight={fieldMax}
          placeholder={m.composer_mensagem_para({ nome: providerLabel })} accessibilityLabel={m.nova_conversa_placeholder()}
          editable={!collapsed} autoFocus={!home && (router.canGoBack?.() ?? true)} recording={dictation.gravando} rms={dictation.rms} onCancelRecording={dictation.cancel}
          onPaste={(files, error) => {
            if (busy || dictation.gravando) return;
            if (error) { setAttachError(formataErro(error) ?? error); return; }
            const f = files[0];
            if (!f) return;
            pick(fromPastedFile(f));
            if (files.length > 1) setAttachError(m.nova_conversa_um_anexo());
          }} />
        </View>
        {dictation.transcribing ? <Text style={styles.meta} accessibilityLiveRegion="polite">{m.composer_transcrevendo_audio()}</Text> : null}
        {dictation.error ? (
          <View style={styles.errorRow}>
            <Text style={[styles.error, styles.flex]} accessibilityRole="alert">{dictation.error}</Text>
            {dictation.canRetry ? (
              <Pressable onPress={dictation.retry} style={styles.retry} accessibilityRole="button">
                <Text style={styles.retryTxt}>{m.composer_transcrever_de_novo()}</Text>
              </Pressable>
            ) : null}
          </View>
        ) : null}
        {attachError ? <Text style={styles.error} accessibilityRole="alert">{attachError}</Text> : null}
      </ComposerBox>
      <View style={styles.underBox} />
      </Animated.View>
      </ComposerDock>
      </Animated.View>
      </Animated.View>
      </ComposerViewport>
      </View>
      </KeyboardAvoidingView>

      <AnchoredPanel open={sheet === 'resume'} anchor={sheetOpener.current} onClose={closeSheet} onDismissed={onSheetDismiss}
        label={m.criar_retomar()} closeLabel={m.native_close()} radius={12} padding={4}>
        <SettingsList items={resume?.entries ?? []} onPick={(id) => { closeSheet(); resume?.onPick(id); }} />
      </AnchoredPanel>

      {/* O menu de pasta do PC (render_compact_folders): painel preso à pílula, como os menus das outras pílulas. */}
      <AnchoredPanel
        open={sheet === 'folder'}
        anchor={folderAnchor.current ?? sheetOpener.current}
        maxHeightRatio={0.75}
        onClose={closeSheet}
        onDismissed={onSheetDismiss}
        label={m.native_new_chat_folder()}
        closeLabel={m.native_close()}
      >
        {sheet === 'folder' ? folderPanel(closeSheet) : null}
      </AnchoredPanel>

      <DictationStyleMenu open={styleMenuOpen} onClose={() => setStyleMenuOpen(false)} server={server} />
      <AttachSheet
        open={attachMenuOpen}
        onClose={() => setAttachMenuOpen(false)}
        onPick={pick}
        onError={setAttachError}
        onPaste={() => {
          readClipboard()
            .then((pasted) => {
              if (pasted && typeof pasted !== 'string') pick(pasted);
              else if (pasted) setText((current) => current + pasted);
              else setAttachError(m.composer_colar_vazio());
            })
            .catch((e: unknown) => setAttachError(e instanceof Error && e.message ? e.message : m.composer_colar_falhou()));
        }}
        dictationStyle={{ label: dictationStyle, onPress: () => setStyleMenuOpen(true) }}
      />
    </View>
  );
}

// Fundo transparente: quem pinta é o Background da Screen, então Aparência (Liso, Textura, Luz,
// Imagem, Transparência) vale aqui como no chat.
// Cinco linhas do corpo (24) mais o respiro de cima e de baixo do campo.
const FIELD_MAX_HEIGHT = 24 * 5 + 16;
// Duas linhas: menos que isso o campo deixa de servir para escrever.
const FIELD_MIN_HEIGHT = 24 * 2 + 16;
// O que o meio guarda com o teclado aberto, para arrastar e fechá-lo. Acima da reserva do
// ComposerDock (96): assim a doca só aperta antes da primeira medida, e a medida nunca sai torta.
const MIDDLE_RESERVE = 120;
// Um alvo de toque: o mínimo que ainda dá para arrastar e baixar o teclado.
const MIDDLE_MIN_RESERVE = 44;
const MARK_SIZE = 84;

const styles = StyleSheet.create((theme) => ({
  root: { flex: 1, backgroundColor: 'transparent' },
  keyboard: { flex: 1 },
  area: { flex: 1 },
  shrink: { flexShrink: 1, minHeight: 0 },
  middle: { flex: 1, minHeight: 44, position: 'relative' },
  homeList: { position: 'absolute', top: 0, left: 0, right: 0, bottom: 0 },
  states: { flex: 1, minHeight: 44 },
  formFrame: { position: 'relative', overflow: 'hidden' },
  measuredForm: { position: 'absolute', left: 0, right: 0, bottom: 0 },
  launcherPosition: { position: 'absolute', left: 0, right: 0, bottom: 0 },
  launcher: { marginHorizontal: theme.base.space[2], borderRadius: theme.base.radius.xl, paddingHorizontal: theme.base.space[3], paddingBottom: theme.base.space[0.5] },
  launcherRow: { flexDirection: 'row', alignItems: 'center', minHeight: 54 },
  launcherText: { flex: 1, minWidth: 0, minHeight: 44, flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2] },
  launcherMic: { minWidth: 44, minHeight: 44, alignItems: 'center', justifyContent: 'center' },
  scroll: { flexGrow: 1, padding: theme.base.space[4], gap: theme.base.space[4] },
  heroArea: { flexGrow: 1 },
  // Na doca, sem espaço, o título rola e encolhe muito antes da caixa: o Enviar fica à vista com o teclado.
  dockHero: { flexGrow: 0, flexShrink: 1000, minHeight: 0 },
  heroSpace: { flexGrow: 1, flexBasis: 0, position: 'relative' },
  // O título da conversa nova, no meio da área livre (ou colado à caixa enquanto falta a pasta).
  topPills: {
    alignItems: 'center',
    gap: theme.base.space[1],
    paddingHorizontal: theme.base.space[4],
    paddingTop: theme.base.space[4],
    paddingBottom: theme.base.space[3],
  },
  // 34 pt + hitSlop 5 = 44 pt de toque; o ícone fica sem fundo, como no PC.
  tool: { width: 34, height: 34, borderRadius: theme.base.radius.full, alignItems: 'center', justifyContent: 'center' },
  dim: { opacity: 0.5 },
  // Onde trabalha, conta e terminal numa linha só, acima da caixa; a conta encolhe com "…".
  optionPills: {
    flexDirection: 'row',
    alignItems: 'center',
    // Respiro entre as pílulas e até a caixa: alvos pequenos colados levam o dedo ao vizinho.
    gap: theme.base.space[3],
    paddingHorizontal: theme.base.space[3],
    paddingBottom: theme.base.space[3],
    minHeight: 34,
  },
  underBox: { height: theme.base.space[2] },
  errorRow: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2], flexWrap: 'wrap' },
  flex: { flex: 1 },
  retry: {
    minHeight: 44,
    justifyContent: 'center',
    borderWidth: 1,
    borderColor: theme.tokens.border.subtle,
    borderRadius: theme.base.radius.full,
    paddingHorizontal: theme.base.space[2],
  },
  retryTxt: { fontSize: theme.base.text.xs, fontWeight: theme.base.weight.semibold, color: theme.tokens.accent.base },
  pending: {
    padding: theme.base.space[3],
    gap: theme.base.space[2],
    borderWidth: 1,
    borderColor: theme.tokens.border.subtle,
    borderRadius: theme.base.radius.md,
    backgroundColor: superficie(theme),
  },
  pendingText: { fontSize: theme.base.text.sm, color: theme.tokens.text.primary },
  pendingAttach: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[1.5] },
  label: { fontSize: theme.base.text.sm, color: theme.tokens.text.secondary, fontWeight: theme.base.weight.medium },
  hint: { fontSize: theme.base.text.sm, color: theme.tokens.text.secondary },
  meta: { fontSize: theme.base.text.xs, color: theme.tokens.text.muted },
  error: { color: theme.tokens.status.error, fontSize: theme.base.text.sm },
  secondary: { minHeight: 44, padding: theme.base.space[2], borderWidth: 1, borderColor: theme.tokens.border.default, borderRadius: theme.base.radius.md, justifyContent: 'center', alignItems: 'center' },
  secondaryTxt: { color: theme.tokens.text.primary, fontSize: theme.base.text.sm, fontWeight: theme.base.weight.medium },
  ghost: { minHeight: 44, padding: theme.base.space[2], justifyContent: 'center', alignItems: 'center' },
  ghostTxt: { color: theme.tokens.text.secondary, fontSize: theme.base.text.sm },
  disabled: { opacity: 0.5 },
}));
