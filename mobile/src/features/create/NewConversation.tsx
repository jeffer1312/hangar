import { useCallback, useEffect, useRef, useState, type ReactNode } from 'react';
import { AccessibilityInfo, ActivityIndicator, AppState, Keyboard, Pressable, ScrollView, Text, TextInput, View } from 'react-native';
import Animated, {
  Easing, useAnimatedStyle, useReducedMotion, useSharedValue, withDelay, withSpring, withTiming,
} from 'react-native-reanimated';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { useRouter } from 'expo-router';
import { KeyboardAvoidingView } from 'react-native-keyboard-controller';
import { SettingsList } from '../../ui/SettingsList';
import { ComposerActions, ComposerBox, ComposerInput } from '../../chat/ComposerField';
import type { Server } from '@hangar/core';
import { AnchoredPanel } from '../../ui/AnchoredPanel';
import { Icon } from '../../ui/Icon';
import { Glass } from '../../ui/Glass';
import type { HomeScreen } from '../../stores/aparencia';
import { AttachmentPreview } from '../../ui/AttachmentPreview';
import { readClipboard, type PickedAttachment } from '../../ui/attachmentPicker';
import { AttachSheet } from '../../ui/AttachSheet';
import { usePressScale } from '../../ui/pressScale';
import { HomeUsageCard } from '../usage/HomeUsageCard';
import { DictationStyleMenu, useDictationStyleLabel } from '../ditado/EstiloPill';
import { removeDraftAttachment, retainDraftAttachment } from '../../chat/draftAttachments';
import { withoutUpload } from '../../stores/drafts';
import { superficie } from '../../theme/superficie';
import {
  adoptCandidate, attemptAttachment, beginAttempt, discardAttempt, recoverAttempt, restoreAttempt, sendFirstInput,
  useNewConversation, type NewConversationInput,
} from '../../stores/newConversation';
import { HomeIntroMark } from './HomeAmbient';
import { useHomeDictation } from './useHomeDictation';
import * as m from '../../paraglide/messages';

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
  // Máquina e pasta, acima da caixa e à direita, como no app de PC.
  topPills: (openFolder: (opener: View | null) => void) => ReactNode;
  // Conta e terminal, abaixo da caixa e à esquerda.
  bottomPills: ReactNode;
  providerChip: ReactNode;
  // Conversas antigas desta pasta que dá para continuar (hoje só o Codex lista).
  resume: { entries: { id: string; label: string }[]; busy: boolean; onPick: (id: string) => void } | null;
  notices: ReactNode;
  // null enquanto falta destino ou conta; o texto continua editável.
  body: NewConversationInput['body'] | null;
  blocked: boolean;
  // Distância fixa do topo da janela até a tela; sem ela o recuo do teclado é medido na janela.
  keyboardOffset?: number;
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
  topPills, bottomPills, providerChip, resume, notices, body, blocked, keyboardOffset, home,
}: Props) {
  const collapsed = home?.mode === 'sessions';
  const input = useRef<TextInput>(null);
  const focusedRequest = useRef(0);
  const router = useRouter();
  const { theme } = useUnistyles();
  const serverId = server.id;
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
  const usageIn = useSharedValue(reduced ? 1 : 0);
  useEffect(() => {
    if (reduced) { rise.value = 1; riseTop.value = 1; usageIn.value = 1; return; }
    rise.value = 0;
    riseTop.value = 0;
    usageIn.value = 0;
    rise.value = withSpring(1, RISE_SPRING);
    riseTop.value = withDelay(80, withSpring(1, RISE_SPRING));
    usageIn.value = withDelay(360, withTiming(1, { duration: 260, easing: Easing.out(Easing.cubic) }));
  }, [intro.epoch, reduced, rise, riseTop, usageIn]);
  const riseStyle = useAnimatedStyle(() => ({
    opacity: Math.min(1, rise.value * 1.4),
    transform: [{ translateY: (1 - rise.value) * 28 }],
  }));
  const riseTopStyle = useAnimatedStyle(() => ({
    opacity: Math.min(1, riseTop.value * 1.4),
    transform: [{ translateY: (1 - riseTop.value) * 20 }],
  }));
  const usageStyle = useAnimatedStyle(() => ({
    opacity: usageIn.value,
    transform: [{ translateY: (1 - usageIn.value) * 8 }],
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

  const [keyboardOpen, setKeyboardOpen] = useState(() => Keyboard.isVisible());
  useEffect(() => {
    const show = Keyboard.addListener('keyboardDidShow', () => setKeyboardOpen(true));
    const hide = Keyboard.addListener('keyboardDidHide', () => setKeyboardOpen(false));
    return () => { show.remove(); hide.remove(); };
  }, []);

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

  const handleSend = async () => {
    if (!body || blocked || busy || !restored || recording || (!text.trim() && !attachment) || submitting.current) return;
    submitting.current = true;
    try {
      await beginAttempt(serverId, { body, text, attachment: attachment ? withoutUpload(attachment) : null });
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

  // Sem pasta escolhida o seletor ocupa o meio; escolhida, o meio fica com a marca, como no PC.
  const showPicker = destinationPending && sheet === null;
  // Com o teclado fechado e o campo vazio, o alto mostra o uso da máquina, como na tela inicial do PC.
  const showUsage = home ? !collapsed : !keyboardOpen && !hasContent;

  return (
    <View style={styles.root}>
      <KeyboardAvoidingView behavior="padding" automaticOffset={keyboardOffset === undefined}
                            keyboardVerticalOffset={keyboardOffset} style={styles.keyboard}>
      <View style={styles.middle}>
      {home ? <Animated.View style={[styles.homeList, listStyle]} pointerEvents={collapsed ? 'auto' : 'none'}
        accessibilityElementsHidden={!collapsed} importantForAccessibility={collapsed ? 'auto' : 'no-hide-descendants'}>
        {home.sessions}
      </Animated.View> : null}
      <Animated.View style={[styles.states, home && conversationStyle]} pointerEvents={collapsed ? 'none' : 'auto'}
        accessibilityElementsHidden={collapsed} importantForAccessibility={collapsed ? 'no-hide-descendants' : 'auto'}>
      <ScrollView style={styles.states} contentContainerStyle={styles.scroll} keyboardShouldPersistTaps="handled">
        {!home && !reduced && !showPicker && !pending ? <HomeIntroMark epoch={intro.epoch} /> : null}
        {showPicker ? destination : null}
        {!showPicker && !pending && showUsage ? (
          <Animated.View style={[styles.usage, !home && usageStyle]}>
            <HomeUsageCard server={server} intro={reduced ? null : intro} />
          </Animated.View>
        ) : null}
        {!showPicker && !pending && !showUsage ? (
          <View style={styles.emptyHint}>
            <Text style={styles.hint}>{m.native_empty_chat_hint({ agent: providerLabel })}</Text>
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

      <Animated.View style={home ? [styles.formFrame, frameStyle] : undefined}>
      {home ? <Animated.View style={[styles.launcherPosition, launcherStyle]} pointerEvents={collapsed ? 'auto' : 'none'}
        accessibilityElementsHidden={!collapsed} importantForAccessibility={collapsed ? 'auto' : 'no-hide-descendants'}
        onLayout={(event) => { launcherHeight.value = event.nativeEvent.layout.height; }}>
        <Glass variant="chrome" style={styles.launcher}>
          <View style={styles.launcherRow}>
            <Pressable style={styles.launcherText} accessibilityRole="button" accessibilityLabel={m.home_new_conversation()}
              onPress={() => home.onNewConversation(true)}>
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
      <Animated.View style={home ? [styles.measuredForm, formStyle] : undefined} pointerEvents={collapsed ? 'none' : 'auto'}
        accessibilityElementsHidden={collapsed} importantForAccessibility={collapsed ? 'no-hide-descendants' : 'auto'}
        onLayout={home ? (event) => { formHeight.value = event.nativeEvent.layout.height; } : undefined}>
      <Animated.View style={[styles.topPills, !home && riseTopStyle]}>{topPills((opener) => openSheet('folder', opener))}</Animated.View>

      <Animated.View style={!home ? riseStyle : undefined}>

      {/* A caixa do app de PC: campo em cima; anexo, conversas antigas, microfone e estilo do
          ditado à esquerda; o chip do provider e o Enviar à direita. */}
      <ComposerBox focused={focused}>
        {attachment ? <AttachmentPreview attachment={attachment} onRemove={removeAttachment} disabled={busy} /> : null}
        <ComposerInput ref={input} value={text} onChangeText={setText} onFocus={() => setFocused(true)} onBlur={() => setFocused(false)}
          placeholder={m.composer_mensagem_para({ nome: providerLabel })} accessibilityLabel={m.nova_conversa_placeholder()}
          editable={!collapsed} autoFocus={!home && (router.canGoBack?.() ?? true)} recording={dictation.gravando} rms={dictation.rms} onCancelRecording={dictation.cancel} />
        {dictation.transcribing ? <Text style={styles.meta} accessibilityLiveRegion="polite">{m.composer_transcrevendo_audio()}</Text> : null}
        <ComposerActions attach={{ onPress: () => setAttachMenuOpen(true), disabled: busy || dictation.gravando }}
          mic={{ onPress: () => void dictation.toggle(), disabled: dictation.transcribing || busy, busy: dictation.transcribing, showBusy: true,
            recording: dictation.gravando, styleLabel: dictationStyle, onStyle: () => setStyleMenuOpen(true) }}
          settings={providerChip}
          beforeMic={resume ? <View ref={resumeAnchor}><HistoryButton busy={resume.busy}
            disabled={busy || resume.busy} onPress={() => openSheet('resume', resumeAnchor.current)} /></View> : null}
          send={{ label: m.nova_conversa_enviar(), disabled: !canSend, busy, showBusy: true, onPress: () => void handleSend() }} />
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
      <View style={styles.bottomPills}>{bottomPills}</View>
      </Animated.View>
      </Animated.View>
      </Animated.View>
      </KeyboardAvoidingView>

      <AnchoredPanel open={sheet === 'resume'} anchor={sheetOpener.current} onClose={closeSheet} onDismissed={onSheetDismiss}
        label={m.criar_retomar()} closeLabel={m.native_close()} radius={12} padding={4}>
        <SettingsList items={resume?.entries ?? []} onPick={(id) => { closeSheet(); resume?.onPick(id); }} />
      </AnchoredPanel>

      {/* O menu de pasta do PC (render_compact_folders): painel preso à pílula, como os menus das outras pílulas. */}
      <AnchoredPanel
        open={sheet === 'folder'}
        anchor={sheetOpener.current}
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
const styles = StyleSheet.create((theme) => ({
  root: { flex: 1, backgroundColor: 'transparent' },
  keyboard: { flex: 1 },
  middle: { flex: 1, minHeight: 44, position: 'relative' },
  homeList: { position: 'absolute', top: 0, left: 0, right: 0, bottom: 0 },
  states: { flex: 1, minHeight: 44 },
  formFrame: { position: 'relative', overflow: 'hidden' },
  measuredForm: { position: 'absolute', left: 0, right: 0, bottom: 0 },
  launcherPosition: { position: 'absolute', left: 0, right: 0, bottom: 0 },
  launcher: { marginHorizontal: theme.base.space[2], borderRadius: 26, paddingHorizontal: 12, paddingBottom: 2 },
  launcherRow: { flexDirection: 'row', alignItems: 'center', minHeight: 54 },
  launcherText: { flex: 1, minWidth: 0, minHeight: 44, flexDirection: 'row', alignItems: 'center', gap: 10 },
  launcherMic: { minWidth: 44, minHeight: 44, alignItems: 'center', justifyContent: 'center' },
  scroll: { flexGrow: 1, padding: theme.base.space[4], gap: theme.base.space[4] },
  // Como no PC: o uso fica no alto, longe da caixa de mensagem.
  usage: { paddingTop: theme.base.space[2] },
  emptyHint: { flexGrow: 1, minHeight: 140, alignItems: 'center', justifyContent: 'center', paddingHorizontal: theme.base.space[5] },
  topPills: {
    flexDirection: 'row',
    justifyContent: 'flex-end',
    alignItems: 'center',
    gap: 2,
    paddingHorizontal: theme.base.space[3],
    paddingBottom: 6,
  },
  // 34 pt + hitSlop 5 = 44 pt de toque; o ícone fica sem fundo, como no PC.
  tool: { width: 34, height: 34, borderRadius: 17, alignItems: 'center', justifyContent: 'center' },
  dim: { opacity: 0.5 },
  bottomPills: {
    flexDirection: 'row',
    flexWrap: 'wrap',
    alignItems: 'center',
    gap: 2,
    paddingHorizontal: theme.base.space[3],
    paddingTop: 4,
    paddingBottom: theme.base.space[2],
    minHeight: 34,
  },
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
  retryTxt: { fontSize: theme.base.text.xs, fontWeight: '600', color: theme.tokens.accent.base },
  pending: {
    padding: theme.base.space[3],
    gap: theme.base.space[2],
    borderWidth: 1,
    borderColor: theme.tokens.border.subtle,
    borderRadius: theme.base.radius.md,
    backgroundColor: superficie(theme),
  },
  pendingText: { fontSize: theme.base.text.sm, color: theme.tokens.text.primary },
  pendingAttach: { flexDirection: 'row', alignItems: 'center', gap: 6 },
  label: { fontSize: theme.base.text.sm, color: theme.tokens.text.secondary, fontWeight: '500' },
  hint: { fontSize: theme.base.text.sm, color: theme.tokens.text.secondary },
  meta: { fontSize: theme.base.text.xs, color: theme.tokens.text.muted },
  error: { color: theme.tokens.status.error, fontSize: theme.base.text.sm },
  secondary: { minHeight: 44, padding: theme.base.space[2], borderWidth: 1, borderColor: theme.tokens.border.default, borderRadius: theme.base.radius.md, justifyContent: 'center', alignItems: 'center' },
  secondaryTxt: { color: theme.tokens.text.primary, fontSize: theme.base.text.sm, fontWeight: '500' },
  ghost: { minHeight: 44, padding: theme.base.space[2], justifyContent: 'center', alignItems: 'center' },
  ghostTxt: { color: theme.tokens.text.secondary, fontSize: theme.base.text.sm },
  disabled: { opacity: 0.5 },
}));
