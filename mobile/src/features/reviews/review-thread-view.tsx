import { memo, useEffect, useRef, useState } from 'react';
import { ScrollView, Text, TextInput, View } from 'react-native';
import { KeyboardAvoidingView } from 'react-native-keyboard-controller';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import type { ReviewFile, ReviewThread } from '@hangar/core';
import { AssistantBubble } from '../../chat/AssistantBubble';
import { Sheet } from '../../ui/Sheet';
import { Chip } from '../../ui/Chip';
import { PatchLine } from '../git/patch-line';
import { ReviewButton, styles } from './review-ui';
import * as m from '../../paraglide/messages';

function Patch({ text }: { text: string }) {
  const [expanded, setExpanded] = useState(false);
  const lines = text.split('\n');
  return <View><ScrollView horizontal><View>{lines.slice(0, expanded ? undefined : 150).map((line, index) => <PatchLine key={index} line={line} />)}</View></ScrollView>
    {!expanded && lines.length > 150 ? <ReviewButton label={m.tool_mostrar_mais_linhas({ n: lines.length - 150 })} onPress={() => setExpanded(true)} /> : null}</View>;
}

export const ReviewThreadView = memo(function ReviewThreadView({ thread, serverId, busy, onReply, onResolve, onCopy }: {
  thread: ReviewThread; serverId: string; busy: boolean; onReply: (thread: ReviewThread) => void;
  onResolve: (thread: ReviewThread) => void; onCopy: (thread: ReviewThread) => void;
}) {
  const [code, setCode] = useState(false);
  return <View style={local.thread}>
    <View style={styles.wrap}><Text selectable style={styles.meta}>{thread.file ? `${thread.file}${thread.line ? `:${thread.line}` : ''}` : m.native_review_thread_general()}</Text>
      {thread.resolved ? <Chip>{m.native_review_thread_resolved()}</Chip> : null}</View>
    {thread.code ? <ReviewButton label={m.mobile_review_code()} icon="Code" onPress={() => setCode((value) => !value)} /> : null}
    {code ? <Patch text={thread.code} /> : null}
    {thread.comments.map((comment, index) => <View key={index} style={local.comment}>
      <Text style={styles.heading}>{comment.author}</Text>
      <AssistantBubble text={comment.body} serverId={serverId} />
    </View>)}
    <View style={styles.wrap}>
      <ReviewButton label={m.native_review_reply()} icon="MessageSquare" disabled={busy} onPress={() => onReply(thread)} />
      {!thread.resolved ? <ReviewButton label={m.native_review_resolve()} icon="Check" disabled={busy} onPress={() => onResolve(thread)} /> : null}
      <ReviewButton label={m.native_review_copy()} icon="Copy" onPress={() => onCopy(thread)} />
    </View>
  </View>;
});

export function ReviewFileView({ file }: { file: ReviewFile }) {
  const [open, setOpen] = useState(false);
  return <View style={local.thread}>
    <ReviewButton label={`${file.path}  +${file.additions} −${file.deletions}`} icon="FileText" onPress={() => setOpen((value) => !value)} />
    {open ? file.patch ? <Patch text={file.patch} /> : <Text style={styles.muted}>{m.native_review_file_no_patch()}</Text> : null}
  </View>;
}

export function ReviewReplySheet({ thread, onClose, onSend, error }: { thread: ReviewThread; onClose: () => void; onSend: (body: string) => Promise<boolean>; error: string }) {
  const { theme } = useUnistyles();
  const input = useRef<TextInput>(null);
  const [text, setText] = useState('');
  const [busy, setBusy] = useState(false);
  const alive = useRef(true);
  useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);
  const send = async () => {
    const body = text.trim();
    if (!body || busy) return;
    setBusy(true);
    const done = await onSend(body);
    if (!alive.current) return;
    setBusy(false);
    if (done) onClose();
  };
  return <Sheet open onDismiss={onClose} onDidPresent={() => input.current?.focus()} sizes={['large']} scrollable dismissible={!busy}>
    <KeyboardAvoidingView behavior="padding" automaticOffset style={styles.page}>
      <ScrollView contentContainerStyle={styles.content} keyboardShouldPersistTaps="handled">
        <Text style={styles.title}>{m.native_review_reply()}</Text>
        <Text style={styles.meta}>{thread.file || m.native_review_thread_general()}</Text>
        <TextInput ref={input} value={text} onChangeText={setText} multiline editable={!busy} accessibilityLabel={m.native_review_reply_placeholder()}
          placeholderTextColor={theme.tokens.text.muted}
          placeholder={m.native_review_reply_placeholder()} style={[styles.search, local.input]} textAlignVertical="top" />
        {error ? <Text accessibilityRole="alert" style={styles.error}>{error}</Text> : null}
        <View style={styles.wrap}>
          <ReviewButton label={m.native_send()} icon="Send" primary disabled={!text.trim() || busy} onPress={() => { void send(); }} />
          <ReviewButton label={m.native_cancel()} disabled={busy} onPress={onClose} />
        </View>
      </ScrollView>
    </KeyboardAvoidingView>
  </Sheet>;
}

const local = StyleSheet.create((theme) => ({
  thread: { paddingVertical: theme.base.space[4], gap: theme.base.space[3], borderBottomWidth: StyleSheet.hairlineWidth, borderBottomColor: theme.tokens.border.default },
  comment: { gap: theme.base.space[2] }, input: { minHeight: 160, paddingTop: theme.base.space[3] },
}));
