import { memo, useMemo, useRef, useState, type ReactNode } from 'react';
import { Pressable, ScrollView, Text, TextInput, View, type StyleProp, type TextStyle } from 'react-native';
import { Image } from 'expo-image';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import {
  answerWhatsAppDraft, draftAvatarUri, draftCharCount, draftClock, draftField, draftInitial, draftRefusalEndsCard, draftSecondsLeft,
  draftTextError, fmtBytes, formatCountdown, getWhatsApp, invisibleMark, markInvisibles, normalizeDraftText, recentBubble, recentSpeaker,
  sendEditedWhatsAppDraft, sentTextMatches, whatsappDraftRefusalText,
  type Server, type WhatsAppDraft, type WhatsAppDraftAnswer, type WhatsAppDraftRecent, type WhatsAppOutboxEvent,
} from '@hangar/core';
import { MachineGroup, MachineRow } from '../machines/MachineRows';
import { Notice } from '../machines/parts';
import { useSettingsColors, withAlpha } from '../config/colors';
import { codeOf } from '../config/whatsapp';
import { ownerRequests } from '../requests/ownerRequests';
import { useSessions } from '../../stores/sessions';
import { openWhatsAppDraft, useWhatsAppTarget } from './target';
import { toast } from '../../ui/Toast';
import { haptic } from '../../ui/haptics';
import { useNow } from '../../ui/useNow';
import { MONO } from '../../theme/scale';
import * as m from '../../paraglide/messages';

const SOURCE: Record<string, () => string> = {
  contact: m.whatsapp_draft_source_contact, push: m.whatsapp_draft_source_push, group: m.whatsapp_draft_source_group,
};

/** Destino como o aviso fala dele: o número primeiro, porque o nome pode ser o que a pessoa escolheu. */
const chatTitle = (d: WhatsAppDraft) => [d.chat_number, d.chat_name].filter(Boolean).join(' · ');

const outbox = ownerRequests<WhatsAppOutboxEvent, WhatsAppDraft>({
  select: (s) => s.whatsappOutbox,
  items: (event) => event.drafts,
  useTarget: () => {
    const target = useWhatsAppTarget((s) => s.target);
    return target && { serverId: target.serverId, id: target.draftId };
  },
  setTarget: (target) => openWhatsAppDraft(target && { serverId: target.serverId, draftId: target.id }),
  icon: 'MessageCircle',
  hint: () => m.whatsapp_draft_open(),
  machineTitle: (p) => m.whatsapp_draft_requests_title(p),
  sessionTitle: (d) => m.whatsapp_draft_session_notice({ name: chatTitle(d) }),
  sheet: {
    title: () => m.whatsapp_draft_title(), noServer: () => m.whatsapp_draft_no_server(), loading: () => m.whatsapp_draft_loading(),
    gone: () => m.whatsapp_draft_gone(), next: () => m.whatsapp_draft_next(),
  },
  Card: DraftCard,
});

/** Aviso por máquina com rascunho esperando (a máquina do aviso é a do WhatsApp). */
export const OutboxRequests = outbox.Notices;
/** Botão com a contagem, para a barra do tablet (ao lado do do cofre). */
export const OutboxRequestsButton = outbox.CountButton;
/** Rascunho pedido pela sessão aberta: aparece no chat dela. */
export const SessionOutboxRequests = outbox.SessionNotices;
/** A folha que responde, na raiz do app: abre da lista, do chat e do push. */
export const OutboxRequestSheet = outbox.Sheet;

function DraftCard({ server, item: draft, onDone }: { server: Server; item: WhatsAppDraft; onDone: () => void }) {
  // Só o hangar-server que lê a edição no serviço manda `edit: true`; sem ele, o cartão não oferece "Editar".
  const edit = useSessions((s) => s.whatsappOutbox?.[server.id]?.edit === true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  // O `busy` só chega no próximo render: dois toques no mesmo quadro passariam os dois.
  const inFlight = useRef(false);

  const answer = async (kind: WhatsAppDraftAnswer, editedText?: string) => {
    if (inFlight.current) return;
    inFlight.current = true;
    setBusy(true);
    setError('');
    try {
      let confirmed = true;
      if (kind === 'send' && editedText !== undefined) {
        confirmed = sentTextMatches(await sendEditedWhatsAppDraft(server, draft.id, editedText, 'mobile'), editedText);
      } else await answerWhatsAppDraft(server, draft.id, kind, 'mobile');
      outbox.answered.add(draft.id);
      // O serviço aprovou outro texto: nunca dizer que saiu o editado. O aviso dura além do cartão, que sai com o envio.
      if (!confirmed) toast.erro(m.whatsapp_draft_err_sent_text_mismatch());
      else toast.ok(kind === 'send' ? m.whatsapp_draft_sending() : m.whatsapp_draft_declined());
      onDone();
    } catch (e) {
      const code = codeOf(e) ?? undefined;
      const fallback = e instanceof Error ? e.message : String(e);
      // Expirado, respondido em outra tela ou apagado não volta: o cartão sai e o motivo vai no aviso.
      if (draftRefusalEndsCard(code)) {
        outbox.answered.add(draft.id);
        toast.erro(whatsappDraftRefusalText(code) ?? fallback);
        onDone();
        return;
      }
      // A trava diz até quando pelo status do serviço; sem ele, a frase sem data.
      const until = code === 'whatsapp_reachout_locked'
        ? await getWhatsApp(server).then((s) => s.status?.reachout_locked_until ?? null, () => null) : null;
      setError(whatsappDraftRefusalText(code, until) ?? fallback);
      haptic.error();
    } finally {
      inFlight.current = false;
      setBusy(false);
    }
  };

  return <DraftCardView draft={draft} edit={edit} busy={busy} error={error} onAnswer={(kind, text) => void answer(kind, text)} />;
}

/** Tempo até o rascunho deixar de aceitar resposta: só esta linha redesenha a cada segundo. `now`: hora fixa (a novidade da
 *  atualização), sem relógio. */
const Countdown = memo(function Countdown({ draft, now }: { draft: WhatsAppDraft; now?: number }) {
  const ticking = useNow(now === undefined);
  return <MachineRow title={m.whatsapp_draft_expires()} value={formatCountdown(draftSecondsLeft(draft, now ?? ticking))} />;
});

/** O cartão do rascunho, só desenho e o campo da edição: a folha passa o estado e a resposta; a novidade da atualização, um
 *  quadro do roteiro com a hora fixa em `now`. `edit`: o serviço aceita texto editado. `onAnswer` recebe o texto do campo,
 *  normalizado, só quando ele difere do rascunho. */
export function DraftCardView({ draft, edit = false, now, busy, error, onAnswer }: {
  draft: WhatsAppDraft;
  edit?: boolean;
  now?: number;
  busy: boolean;
  error: string;
  onAnswer: (kind: WhatsAppDraftAnswer, editedText?: string) => void;
}) {
  const { theme } = useUnistyles();
  const c = useSettingsColors();
  const warn = theme.tokens.status.warning;
  const source = SOURCE[draft.chat_name_source]?.();
  const file = draft.file;
  // Campo aberto não fecha se o evento passar a dizer `edit: false`: fechar faria sair o original; o servidor recusa com 409.
  const [editing, setEditing] = useState(false);
  const [field, setField] = useState(draft.text);
  const typed = useMemo(() => draftField(field), [field]);
  // O campo mostra o texto cru: o que a leitura marcaria vai junto sem aparecer.
  const hidden = useMemo(() => markInvisibles(typed.text).filter((p) => p.kind === 'mark').length, [typed.text]);
  const original = useMemo(() => normalizeDraftText(draft.text), [draft.text]);
  const changed = editing && typed.text !== original;
  // Só o texto mudado vai pela edição; o igual sai pelo Enviar de sempre, que já passou pelas regras do serviço.
  const fieldError = changed ? draftTextError(typed, !!file) : undefined;
  const editingLabel = m.whatsapp_draft_text_editing({ count: typed.count });
  const toggle = () => {
    haptic.selection();
    setField(draft.text);
    setEditing(!editing);
  };
  return <>
    <MachineGroup>
      <Block label={m.whatsapp_draft_to()}>
        <View style={styles.dest}>
          <Avatar draft={draft} />
          <View style={styles.destText}>
            <Text selectable style={[styles.number, { color: c.text }]}>{draft.chat_number}</Text>
            {draft.chat_name || source ? <Text style={[styles.small, { color: c.muted }]}>{[draft.chat_name, source].filter(Boolean).join(' · ')}</Text> : null}
            {draft.chat_name_source === 'push' ? <Text style={[styles.small, { color: warn }]}>{m.whatsapp_draft_push_warning()}</Text> : null}
          </View>
        </View>
      </Block>
      <MachineRow title={m.whatsapp_draft_origin()} value={draft.origin} />
      <Countdown draft={draft} now={now} />
    </MachineGroup>
    {draft.recent.length || draft.quoted_text ? <MachineGroup>
      {draft.recent.length ? <Block label={m.whatsapp_draft_recent()}>
        <View style={[styles.chat, { backgroundColor: c.inset }]}>
          {draft.recent.map((r, i) => <Bubble key={i} recent={r} name={recentSpeaker(draft, r)} />)}
        </View>
      </Block> : null}
      {draft.quoted_text ? <Block label={m.whatsapp_draft_quoted()} accent={c.accent}>
        <Text style={[styles.line, { color: c.text }]}>{draft.quoted_text}</Text>
      </Block> : null}
    </MachineGroup> : null}
    {/* Como no nativo: rascunho só com anexo também ganha "Editar" para receber um texto. */}
    {draft.text || edit || editing ? <MachineGroup>
      <Block label={editing ? editingLabel : m.whatsapp_draft_text({ count: draftCharCount(draft.text) })}
        action={edit || editing ? <Pressable accessibilityRole="button" accessibilityLabel={editing ? m.whatsapp_draft_edit_undo() : m.whatsapp_draft_edit()}
          disabled={busy} hitSlop={LINK_SLOP} onPress={toggle}>
          <Text style={[styles.small, { color: c.accent }]}>{editing ? m.whatsapp_draft_edit_undo() : m.whatsapp_draft_edit()}</Text>
        </Pressable> : null}>
        {editing ? <TextInput multiline value={field} onChangeText={setField} editable={!busy} autoFocus
          accessibilityLabel={editingLabel}
          style={[styles.mono, styles.field, { color: c.text, backgroundColor: c.inset, borderColor: warn }]} />
          : null}
        {editing && hidden ? <Text style={[styles.small, { color: warn }]}>{m.whatsapp_draft_edit_invisibles({ count: hidden })}</Text> : null}
        {editing ? null : draft.text ? <ScrollView nestedScrollEnabled style={styles.textBox}>
            <Marked text={draft.text} selectable style={[styles.mono, { color: c.text }]} />
          </ScrollView> : null}
      </Block>
    </MachineGroup> : null}
    {file ? <MachineGroup>
      <MachineRow title={m.whatsapp_draft_attachment()} value={`${file.name} · ${fmtBytes(file.size)}`} />
      <Block label={m.whatsapp_draft_path()}>
        <Text selectable style={[styles.mono, { color: file.outside_cwd ? warn : c.muted }]}>{file.source}</Text>
        {file.outside_cwd ? <Text style={[styles.small, { color: warn }]}>{m.whatsapp_draft_outside_cwd()}</Text> : null}
      </Block>
    </MachineGroup> : null}
    <Notice text={fieldError ?? error} error />
    <MachineGroup>
      <MachineRow title={changed ? m.whatsapp_draft_send_edited() : m.whatsapp_draft_send()} action disabled={busy || !!fieldError}
        onPress={() => (changed ? onAnswer('send', typed.text) : onAnswer('send'))} />
      <MachineRow title={m.whatsapp_draft_decline()} danger disabled={busy} onPress={() => onAnswer('decline')} />
    </MachineGroup>
  </>;
}

// Com a linha de 19 do rótulo, o toque no link chega aos 44 pt.
const LINK_SLOP = { top: 13, bottom: 13, left: 8, right: 8 };

/** Foto do destino; sem ela, ou com bytes que não abrem, a inicial do nome num círculo do mesmo tamanho. */
const Avatar = memo(function Avatar({ draft }: { draft: WhatsAppDraft }) {
  const c = useSettingsColors();
  const [broken, setBroken] = useState(false);
  // Fonte estável: o base64 da foto não vira objeto novo a cada desenho do cartão.
  const source = useMemo(() => (draft.chat_avatar ? { uri: draftAvatarUri(draft.chat_avatar) } : null), [draft.chat_avatar]);
  if (source && !broken) {
    return <Image source={source} alt={m.whatsapp_draft_avatar({ name: draft.chat_name || draft.chat_number })}
      contentFit="cover" onError={() => setBroken(true)} style={[styles.avatar, { borderColor: c.borderStrong }]} />;
  }
  return <View accessibilityElementsHidden importantForAccessibility="no-hide-descendants"
    style={[styles.avatar, styles.initial, { backgroundColor: c.accentDim, borderColor: c.borderStrong }]}>
    <Text style={[styles.initialText, { color: c.accentText }]}>{draftInitial(draft)}</Text>
  </View>;
});

/** Mensagem recente como balão: as minhas à direita, as dele à esquerda, com a hora (e o nome dele) embaixo. */
const Bubble = memo(function Bubble({ recent: r, name }: { recent: WhatsAppDraftRecent; name: string }) {
  const { theme } = useUnistyles();
  const c = useSettingsColors();
  const { tag, text } = recentBubble(r);
  const clock = draftClock(r.at);
  const meta = r.from_me ? clock : [name, clock].filter(Boolean).join(' · ');
  // O leitor de tela não vê o lado do balão: quem escreveu vai no rótulo.
  const label = [r.from_me ? m.whatsapp_draft_me() : name, clock, tag, text].filter(Boolean).join(', ');
  return <View accessible accessibilityLabel={label}
    style={[styles.bubble, r.from_me
      ? [styles.mine, { backgroundColor: withAlpha(theme.tokens.status.success, 0.18) }]
      : [styles.theirs, { backgroundColor: theme.tokens.bubbleUser }]]}>
    <View style={styles.media}>
      {tag ? <View style={[styles.tag, { backgroundColor: c.borderStrong }]}>
        <Text style={[styles.tagText, { color: c.muted }]}>{tag}</Text>
      </View> : null}
      <Marked text={text} style={[styles.line, { color: tag && !r.text.trim() ? c.muted : c.text }]} />
    </View>
    {meta ? <Text style={[styles.meta, { color: c.faint }]}>{meta}</Text> : null}
  </View>;
});

/** Texto de terceiro ou do rascunho com os invisíveis marcados. */
function Marked({ text, style, selectable }: { text: string; style: StyleProp<TextStyle>; selectable?: boolean }) {
  const { theme } = useUnistyles();
  const warn = theme.tokens.status.warning;
  const parts = useMemo(() => markInvisibles(text), [text]);
  return <Text selectable={selectable} style={style}>
    {parts.map((p, i) => p.kind === 'mark'
      ? <Text key={i} style={{ color: warn, backgroundColor: withAlpha(warn, 0.14) }}>{invisibleMark(p.code)}</Text>
      : p.text)}
  </Text>;
}

/** Linha em coluna das listas de ajustes: rótulo pequeno em cima e o conteúdo embaixo; `accent` é a faixa da citação e `action`
 *  fica à direita do rótulo. */
function Block({ label, accent, action, children }: { label: string; accent?: string; action?: ReactNode; children: ReactNode }) {
  const c = useSettingsColors();
  return <View style={[styles.block, { borderTopColor: c.border }, accent ? { borderLeftWidth: 3, borderLeftColor: accent } : null]}>
    <View style={styles.labelRow}>
      <Text style={[styles.small, styles.label, { color: c.muted }]}>{label}</Text>
      {action}
    </View>
    {children}
  </View>;
}

const AVATAR = 40;

const styles = StyleSheet.create((theme) => ({
  block: { paddingHorizontal: theme.base.space[4], paddingVertical: theme.base.space[3], gap: theme.base.space[1], borderTopWidth: 1 },
  labelRow: { flexDirection: 'row', alignItems: 'baseline', justifyContent: 'space-between', gap: theme.base.space[3] },
  label: { flexShrink: 1 },
  small: { fontSize: theme.base.text.xs, lineHeight: 19 },
  number: { fontFamily: MONO, fontSize: theme.base.text.base, fontWeight: theme.base.weight.semibold },
  line: { fontSize: theme.base.text.sm, flexShrink: 1 },
  mono: { fontFamily: MONO, fontSize: theme.base.text.sm },
  textBox: { maxHeight: 150 },
  field: {
    minHeight: 120, borderWidth: 1, borderRadius: theme.base.radius.xs, textAlignVertical: 'top',
    paddingHorizontal: theme.base.space[2], paddingVertical: theme.base.space[1.5],
  },
  dest: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2] },
  destText: { flex: 1, minWidth: 0, gap: theme.base.space[0.5] },
  avatar: { width: AVATAR, height: AVATAR, borderRadius: theme.base.radius.full, borderWidth: 1 },
  initial: { alignItems: 'center', justifyContent: 'center' },
  initialText: { fontSize: theme.base.text.sm, fontWeight: theme.base.weight.semibold },
  chat: { gap: theme.base.space[1], padding: theme.base.space[1.5], borderRadius: theme.base.radius.sm },
  bubble: { maxWidth: '82%', paddingHorizontal: theme.base.space[2], paddingVertical: theme.base.space[1], borderRadius: theme.base.radius.sm },
  mine: { alignSelf: 'flex-end', borderBottomRightRadius: theme.base.radius.xs },
  theirs: { alignSelf: 'flex-start', borderBottomLeftRadius: theme.base.radius.xs },
  media: { flexDirection: 'row', flexWrap: 'wrap', alignItems: 'center', gap: theme.base.space[1] },
  tag: { borderRadius: theme.base.radius.xs, paddingHorizontal: theme.base.space[1] },
  tagText: { fontSize: theme.base.text.xxxs, fontWeight: theme.base.weight.semibold },
  meta: { fontSize: theme.base.text.xxxs, lineHeight: 15 },
}));
