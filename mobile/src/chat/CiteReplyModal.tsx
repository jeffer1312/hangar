import { useEffect, useMemo, useRef, useState } from 'react';
import { Keyboard, Modal, Platform, Pressable, ScrollView, Text, TextInput, View, type NativeSyntheticEvent, type TextInputKeyPressEventData } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { KeyboardAvoidingView } from 'react-native-keyboard-controller';
import { EnrichedMarkdownText } from 'react-native-enriched-markdown';
import { codeLabel, elementLabel, withContextRefComment } from '@hangar/core';
import * as m from '../paraglide/messages';
import { mkMutedMarkdownStyle } from './AssistantBubble';
import { addContextRef } from '../stores/contextRefs';
import { closeCiteDialog, useCiteDialog, type CiteRequest } from '../stores/citeDialog';
import { superficie } from '../theme/superficie';

// "Responder ao trecho": o trecho citado (rolagem própria) e a resposta da pessoa. "Comentar e
// continuar" põe na bandeja e devolve a conversa ao mesmo ponto; "Finalizar" põe na bandeja e leva
// ao campo de mensagem (`onFinish`). Quem devolve a conversa ao lugar é a MessageList.
export function CiteReplyModal({ draftKey, onFinish }: { draftKey: string; onFinish: () => void }) {
  const request = useCiteDialog((s) => (s.request?.draftKey === draftKey ? s.request : null));
  if (!request) return null;
  return (
    <Modal visible transparent animationType="fade" statusBarTranslucent onRequestClose={() => closeCiteDialog()}>
      <KeyboardAvoidingView behavior="padding" style={styles.backdrop}>
        {/* Cada abertura monta o cartão de novo: o comentário nasce do que a citação já trazia. */}
        <CiteReplyCard key={request.id} request={request} onFinish={onFinish} />
      </KeyboardAvoidingView>
    </Modal>
  );
}

function CiteReplyCard({ request, onFinish }: { request: CiteRequest; onFinish: () => void }) {
  const { theme } = useUnistyles();
  const [comment, setComment] = useState(request.ref.comment);
  const md = useMemo(() => mkMutedMarkdownStyle(theme), [theme]);
  const input = useRef<TextInput>(null);

  // No Android o `autoFocus` dentro do Modal foca sem abrir o teclado: o pedido vai depois que a janela
  // do modal assume, e de novo se o teclado ainda não subiu (o mesmo do Finalizar no campo de mensagem).
  useEffect(() => {
    const first = setTimeout(() => input.current?.focus(), 300);
    const second = setTimeout(() => {
      if (Keyboard.isVisible()) return;
      input.current?.blur();
      input.current?.focus();
    }, 900);
    return () => { clearTimeout(first); clearTimeout(second); };
  }, []);

  const addToTray = (finish: boolean) => {
    addContextRef(request.draftKey, withContextRefComment(request.ref, comment.trim()));
    closeCiteDialog(finish);
    if (finish) onFinish();
  };

  // Atalhos só com teclado de verdade, que no app só existe no web: no aparelho o Enter quebra linha
  // e as ações ficam nos botões. O shift/ctrl vem do evento do DOM que o react-native-web repassa.
  const onKey = (ev: NativeSyntheticEvent<TextInputKeyPressEventData>) => {
    if (Platform.OS !== 'web' || ev.nativeEvent.key !== 'Enter') return;
    const native = ev.nativeEvent as TextInputKeyPressEventData & { shiftKey?: boolean; ctrlKey?: boolean; metaKey?: boolean };
    if (native.shiftKey) return;
    ev.preventDefault();
    addToTray(!!(native.ctrlKey || native.metaKey));
  };

  const ref = request.ref;
  return (
    <View style={[styles.card, { backgroundColor: superficie(theme, 1), borderColor: theme.tokens.border.default }]}
          accessibilityViewIsModal accessibilityLabel={m.cite_reply_title()}>
      <Text style={[styles.title, { color: theme.tokens.text.primary }]} accessibilityRole="header">{m.cite_reply_title()}</Text>
      <ScrollView style={[styles.excerpt, { borderColor: theme.tokens.accent.base }]} nestedScrollEnabled>
        {ref.kind === 'quote'
          ? <EnrichedMarkdownText markdown={ref.text} markdownStyle={md} flavor="github" />
          : ref.kind === 'code'
            ? (
              <>
                <Text style={{ color: theme.tokens.text.primary }}>{codeLabel(ref)}</Text>
                <Text style={[styles.code, { color: theme.tokens.text.secondary }]}>{ref.text}</Text>
              </>
            )
            : <Text style={{ color: theme.tokens.text.primary }}>{elementLabel(ref)}</Text>}
      </ScrollView>
      <TextInput
        ref={input}
        autoFocus
        multiline
        value={comment}
        onChangeText={setComment}
        onKeyPress={onKey}
        placeholder={m.cite_reply_placeholder()}
        placeholderTextColor={theme.tokens.text.muted}
        accessibilityLabel={m.cite_reply_placeholder()}
        style={[styles.field, { color: theme.tokens.text.primary, borderColor: theme.tokens.border.default }]}
      />
      <View style={styles.actions}>
        <Pressable onPress={() => closeCiteDialog()} style={[styles.btn, { borderColor: theme.tokens.border.default }]} accessibilityRole="button">
          <Text style={[styles.btnTxt, { color: theme.tokens.text.primary }]}>{m.comum_cancelar()}</Text>
        </Pressable>
        <Pressable onPress={() => addToTray(true)} style={[styles.btn, { borderColor: theme.tokens.border.default }]} accessibilityRole="button">
          <Text style={[styles.btnTxt, { color: theme.tokens.text.primary }]}>{m.cite_finish()}</Text>
        </Pressable>
        <Pressable onPress={() => addToTray(false)} style={[styles.btn, { backgroundColor: theme.tokens.accent.base, borderColor: theme.tokens.accent.base }]} accessibilityRole="button">
          <Text style={[styles.btnTxt, { color: theme.tokens.bg.base }]}>{m.cite_comment_continue()}</Text>
        </Pressable>
      </View>
    </View>
  );
}

const styles = StyleSheet.create((theme) => ({
  backdrop: {
    flex: 1,
    justifyContent: 'center',
    padding: theme.base.space[4],
    backgroundColor: 'rgba(4,6,10,0.6)',
  },
  card: {
    borderWidth: 1,
    borderRadius: theme.base.radius.lg,
    padding: theme.base.space[4],
    gap: theme.base.space[3],
    maxHeight: '90%',
  },
  title: { fontSize: theme.base.text.base, fontWeight: theme.base.weight.semibold },
  code: { fontFamily: theme.base.fontMono, fontSize: theme.base.text.xs },
  excerpt: { flexGrow: 0, maxHeight: 220, borderLeftWidth: 3, paddingLeft: theme.base.space[2] },
  field: {
    minHeight: 88,
    maxHeight: 200,
    borderWidth: 1,
    borderRadius: theme.base.radius.md,
    paddingHorizontal: theme.base.space[3],
    paddingVertical: theme.base.space[2],
    fontSize: theme.base.text.base,
    textAlignVertical: 'top',
  },
  actions: { flexDirection: 'row', flexWrap: 'wrap', justifyContent: 'flex-end', gap: theme.base.space[2] },
  btn: {
    minHeight: 44,
    paddingHorizontal: theme.base.space[3],
    borderRadius: theme.base.radius.full,
    borderWidth: 1,
    justifyContent: 'center',
  },
  btnTxt: { fontSize: theme.base.text.sm, fontWeight: theme.base.weight.semibold },
}));
