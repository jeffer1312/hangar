import { memo, useMemo, useState, type ReactNode } from 'react';
import { ScrollView, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import {
  answerWhatsAppDraft, draftCharCount, draftClock, draftRefusalEndsCard, draftSecondsLeft, fmtBytes, formatCountdown, getWhatsApp,
  invisibleMark, markInvisibles, whatsappDraftRefusalText,
  type Server, type WhatsAppDraft, type WhatsAppDraftAnswer, type WhatsAppOutboxEvent,
} from '@hangar/core';
import { MachineGroup, MachineRow } from '../machines/MachineRows';
import { Notice } from '../machines/parts';
import { useSettingsColors, withAlpha } from '../config/colors';
import { codeOf } from '../config/whatsapp';
import { ownerRequests } from '../requests/ownerRequests';
import { openWhatsAppDraft, useWhatsAppTarget } from './target';
import { toast } from '../../ui/Toast';
import { haptic } from '../../ui/haptics';
import { useNow } from '../../ui/useNow';
import { MONO } from '../../theme/scale';
import * as m from '../../paraglide/messages';

const SOURCE: Record<string, () => string> = {
  contact: m.whatsapp_draft_source_contact, push: m.whatsapp_draft_source_push, group: m.whatsapp_draft_source_group,
};

/** Nome do destino como o aviso fala dele: o nome do arquivo, senão o número. */
const chatTitle = (d: WhatsAppDraft) => d.chat_name || d.chat_number;

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
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');

  const answer = async (kind: WhatsAppDraftAnswer) => {
    if (busy) return;
    setBusy(true);
    setError('');
    try {
      await answerWhatsAppDraft(server, draft.id, kind, 'mobile');
      outbox.answered.add(draft.id);
      toast.ok(kind === 'send' ? m.whatsapp_draft_sending() : m.whatsapp_draft_declined());
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
    } finally { setBusy(false); }
  };

  return <DraftCardView draft={draft} busy={busy} error={error} onAnswer={(kind) => void answer(kind)} />;
}

/** Tempo até o rascunho deixar de aceitar resposta: só esta linha redesenha a cada segundo. `now`: hora fixa (a novidade da
 *  atualização), sem relógio. */
const Countdown = memo(function Countdown({ draft, now }: { draft: WhatsAppDraft; now?: number }) {
  const ticking = useNow(now === undefined);
  return <MachineRow title={m.whatsapp_draft_expires()} value={formatCountdown(draftSecondsLeft(draft, now ?? ticking))} />;
});

/** O cartão do rascunho, só desenho: a folha passa o estado e a resposta; a novidade da atualização, um quadro do roteiro com
 *  a hora fixa em `now`. */
export function DraftCardView({ draft, now, busy, error, onAnswer }: {
  draft: WhatsAppDraft;
  now?: number;
  busy: boolean;
  error: string;
  onAnswer: (kind: WhatsAppDraftAnswer) => void;
}) {
  const { theme } = useUnistyles();
  const c = useSettingsColors();
  const warn = theme.tokens.status.warning;
  const source = SOURCE[draft.chat_name_source]?.();
  const file = draft.file;
  const parts = useMemo(() => markInvisibles(draft.text), [draft.text]);
  return <>
    <MachineGroup>
      <Block label={m.whatsapp_draft_to()}>
        <Text selectable style={[styles.number, { color: c.text }]}>{draft.chat_number}</Text>
        {draft.chat_name || source ? <Text style={[styles.small, { color: c.muted }]}>{[draft.chat_name, source].filter(Boolean).join(' · ')}</Text> : null}
        {draft.chat_name_source === 'push' ? <Text style={[styles.small, { color: warn }]}>{m.whatsapp_draft_push_warning()}</Text> : null}
      </Block>
      <MachineRow title={m.whatsapp_draft_origin()} value={draft.origin} />
      <Countdown draft={draft} now={now} />
    </MachineGroup>
    {draft.recent.length || draft.quoted_text ? <MachineGroup>
      {draft.recent.length ? <Block label={m.whatsapp_draft_recent()}>
        {draft.recent.map((r, i) => <Text key={i} style={[styles.line, { color: c.text }]}>
          <Text style={{ color: c.muted }}>{[r.from_me ? m.whatsapp_draft_me() : r.sender, draftClock(r.at)].filter(Boolean).join(' · ')} </Text>
          {r.text}
        </Text>)}
      </Block> : null}
      {draft.quoted_text ? <Block label={m.whatsapp_draft_quoted()} accent={c.accent}>
        <Text style={[styles.line, { color: c.text }]}>{draft.quoted_text}</Text>
      </Block> : null}
    </MachineGroup> : null}
    {draft.text ? <MachineGroup>
      <Block label={m.whatsapp_draft_text({ count: draftCharCount(draft.text) })}>
        <ScrollView nestedScrollEnabled style={styles.textBox}>
          <Text selectable style={[styles.mono, { color: c.text }]}>
            {parts.map((p, i) => p.kind === 'mark'
              ? <Text key={i} style={{ color: warn, backgroundColor: withAlpha(warn, 0.14) }}>{invisibleMark(p.code)}</Text>
              : p.text)}
          </Text>
        </ScrollView>
      </Block>
    </MachineGroup> : null}
    {file ? <MachineGroup>
      <MachineRow title={m.whatsapp_draft_attachment()} value={`${file.name} · ${fmtBytes(file.size)}`} />
      <Block label={m.whatsapp_draft_path()}>
        <Text selectable style={[styles.mono, { color: file.outside_cwd ? warn : c.muted }]}>{file.source}</Text>
        {file.outside_cwd ? <Text style={[styles.small, { color: warn }]}>{m.whatsapp_draft_outside_cwd()}</Text> : null}
      </Block>
    </MachineGroup> : null}
    <Notice text={error} error />
    <MachineGroup>
      <MachineRow title={m.whatsapp_draft_send()} action disabled={busy} onPress={() => onAnswer('send')} />
      <MachineRow title={m.whatsapp_draft_decline()} danger disabled={busy} onPress={() => onAnswer('decline')} />
    </MachineGroup>
  </>;
}

/** Linha em coluna das listas de ajustes: rótulo pequeno em cima e o conteúdo embaixo; `accent` é a faixa da citação. */
function Block({ label, accent, children }: { label: string; accent?: string; children: ReactNode }) {
  const c = useSettingsColors();
  return <View style={[styles.block, { borderTopColor: c.border }, accent ? { borderLeftWidth: 3, borderLeftColor: accent } : null]}>
    <Text style={[styles.small, { color: c.muted }]}>{label}</Text>
    {children}
  </View>;
}

const styles = StyleSheet.create((theme) => ({
  block: { paddingHorizontal: theme.base.space[4], paddingVertical: theme.base.space[3], gap: theme.base.space[1], borderTopWidth: 1 },
  small: { fontSize: theme.base.text.xs, lineHeight: 19 },
  number: { fontFamily: MONO, fontSize: theme.base.text.base, fontWeight: theme.base.weight.semibold },
  line: { fontSize: theme.base.text.sm },
  mono: { fontFamily: MONO, fontSize: theme.base.text.sm },
  textBox: { maxHeight: 150 },
}));
