import { useEffect, useState, type ReactNode } from 'react';
import { Pressable, ScrollView, Text, View, type StyleProp, type ViewStyle } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import {
  answerWhatsAppDraft, draftCharCount, draftClock, draftFileSize, draftRefusalEndsCard, draftSecondsLeft, formatCountdown, getWhatsApp,
  invisibleMark, markInvisibles, whatsappDraftRefusalText,
  type Server, type ServerBucket, type WhatsAppDraft, type WhatsAppDraftAnswer, type WhatsAppOutboxEvent,
} from '@hangar/core';
import { MachineGroup, MachineRow } from '../machines/MachineRows';
import { MachineSheet, Notice } from '../machines/parts';
import { InfoNotice } from '../config/InfoNotice';
import { useSettingsColors, withAlpha } from '../config/colors';
import { RequestNotice } from '../vault/VaultRequests';
import { openWhatsAppDraft, useWhatsAppTarget } from './target';
import { useSessions } from '../../stores/sessions';
import { useIdentities } from '../pair/identities';
import { Icon } from '../../ui/Icon';
import { toast } from '../../ui/Toast';
import { haptic } from '../../ui/haptics';
import { MONO } from '../../theme/scale';
import * as m from '../../paraglide/messages';

// Respondido aqui some da folha na hora; o evento `whatsapp_outbox` seguinte confirma.
const answered = new Set<string>();
const pending = (drafts: WhatsAppDraft[]) => drafts.filter((d) => !answered.has(d.id));
// Estado de teste montado sem o campo não derruba a lista.
const NONE: Record<string, WhatsAppOutboxEvent> = {};

const SOURCE: Record<string, () => string> = {
  contact: m.whatsapp_draft_source_contact, push: m.whatsapp_draft_source_push, group: m.whatsapp_draft_source_group,
};

/** Nome do destino como o aviso fala dele: o nome do arquivo, senão o número. */
const chatTitle = (d: WhatsAppDraft) => d.chat_name || d.chat_number;

/** Máquinas da lista com o stream de pé: rascunho de máquina caída não aparece (a lista nova traz de volta). */
const liveServers = (byServer: ServerBucket[]) => byServer.filter((b) => !b.error && b.loaded);

/** Aviso por máquina com rascunho esperando (a máquina do aviso é a do WhatsApp). */
export function OutboxRequests({ byServer }: { byServer: ServerBucket[] }) {
  const outbox = useSessions((s) => s.whatsappOutbox) ?? NONE;
  const notices = liveServers(byServer).flatMap((b) => {
    const open = pending(outbox[b.server.id]?.drafts ?? []);
    return open.length ? [{ server: b.server, label: b.label ?? b.server.label, count: open.length }] : [];
  });
  return <>
    {notices.map(({ server, label, count }) => <RequestNotice key={server.id} icon="MessageCircle" hint={m.whatsapp_draft_open()}
      title={m.whatsapp_draft_requests_title({ label, count })} onPress={() => openWhatsAppDraft({ serverId: server.id, draftId: null })} />)}
  </>;
}

/** Botão com a contagem, para a barra do tablet (ao lado do do cofre). */
export function OutboxRequestsButton({ style }: { style?: StyleProp<ViewStyle> }) {
  const { theme } = useUnistyles();
  const outbox = useSessions((s) => s.whatsappOutbox) ?? NONE;
  const byServer = useSessions((s) => s.byServer) ?? [];
  const open = liveServers(byServer)
    .map((b) => ({ server: b.server, label: b.label ?? b.server.label, count: pending(outbox[b.server.id]?.drafts ?? []).length }))
    .filter((b) => b.count > 0);
  if (!open.length) return null;
  // A folha mostra uma máquina por vez: abre a que mais espera, e o rótulo fala dela.
  const first = open.reduce((a, b) => (b.count > a.count ? b : a));
  const count = open.reduce((n, b) => n + b.count, 0);
  return <Pressable accessibilityRole="button" accessibilityLabel={m.whatsapp_draft_requests_title({ label: first.label, count: first.count })}
    accessibilityHint={m.whatsapp_draft_open()} style={[style, styles.button]}
    onPress={() => { haptic.selection(); openWhatsAppDraft({ serverId: first.server.id, draftId: null }); }}>
    <Icon name="MessageCircle" size={20} color={theme.tokens.status.warning} />
    <Text style={[styles.count, { color: theme.tokens.status.warning }]}>{count}</Text>
  </Pressable>;
}

/** Rascunho pedido pela sessão aberta: aparece no chat dela, pela origem `servidor::sessao`. */
export function SessionOutboxRequests({ serverId, name }: { serverId: string; name: string }) {
  const outbox = useSessions((s) => s.whatsappOutbox) ?? NONE;
  const servers = useSessions((s) => s.servers) ?? [];
  const byServer = useSessions((s) => s.byServer) ?? [];
  const id = useIdentities(servers).identities.get(serverId);
  const live = new Set(liveServers(byServer).map((b) => b.server.id));
  // Sem o identificador da máquina (ainda lendo ou falhou), vale o nome da sessão: o rascunho não pode sumir do chat.
  const fromHere = (origin: string) => id ? origin === `${id}::${name}` : origin.endsWith(`::${name}`);
  const mine = Object.entries(outbox).filter(([owner]) => live.has(owner))
    .flatMap(([owner, event]) => pending(event.drafts).filter((d) => fromHere(d.origin)).map((d) => ({ owner, d })));
  return <>
    {mine.map(({ owner, d }) => <RequestNotice key={d.id} icon="MessageCircle" hint={m.whatsapp_draft_open()}
      title={m.whatsapp_draft_session_notice({ name: chatTitle(d) })} onPress={() => openWhatsAppDraft({ serverId: owner, draftId: d.id })} />)}
  </>;
}

/** A folha que responde, na raiz do app: abre da lista, do chat e do push. */
export function OutboxRequestSheet() {
  const outbox = useSessions((s) => s.whatsappOutbox) ?? NONE;
  const byServer = useSessions((s) => s.byServer) ?? [];
  const target = useWhatsAppTarget((s) => s.target);
  if (!target) return null;
  const bucket = byServer.find((b) => b.server.id === target.serverId);
  return <DraftSheet server={bucket?.server ?? null} ready={!!bucket?.loaded && !bucket.error && !!outbox[target.serverId]}
    drafts={pending(outbox[target.serverId]?.drafts ?? [])} draftId={target.draftId} />;
}

function DraftSheet({ server, ready, drafts, draftId }: {
  server: Server | null;
  ready: boolean;
  drafts: WhatsAppDraft[];
  draftId: string | null;
}) {
  const close = () => openWhatsAppDraft(null);
  const draft = draftId ? drafts.find((d) => d.id === draftId) : drafts[0];
  // Respondido o último rascunho aberto pela lista, a folha fecha sozinha.
  useEffect(() => { if (ready && !draftId && !draft) close(); }, [ready, draftId, draft]);
  const next = () => { if (server) openWhatsAppDraft({ serverId: server.id, draftId: null }); };

  return <MachineSheet title={m.whatsapp_draft_title()} onClose={close}>
    {!server ? <Notice text={m.whatsapp_draft_no_server()} error />
      : !ready ? <InfoNotice text={m.whatsapp_draft_loading()} />
      : !draft ? <>
        <InfoNotice text={m.whatsapp_draft_gone()} />
        {drafts.length ? <MachineGroup><MachineRow title={m.whatsapp_draft_next()} action onPress={next} /></MachineGroup> : null}
      </>
      : <DraftCard key={draft.id} server={server} draft={draft} onDone={next} />}
  </MachineSheet>;
}

/** Relógio de um segundo só enquanto o cartão está na tela. */
function useNow(): number {
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, []);
  return now;
}

function DraftCard({ server, draft, onDone }: { server: Server; draft: WhatsAppDraft; onDone: () => void }) {
  const { theme } = useUnistyles();
  const c = useSettingsColors();
  const now = useNow();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const warn = theme.tokens.status.warning;

  const answer = async (kind: WhatsAppDraftAnswer) => {
    if (busy) return;
    setBusy(true);
    setError('');
    try {
      await answerWhatsAppDraft(server, draft.id, kind, 'mobile');
      answered.add(draft.id);
      toast.ok(kind === 'send' ? m.whatsapp_draft_sending() : m.whatsapp_draft_declined());
      onDone();
    } catch (e) {
      const raw = (e as { code?: unknown } | null)?.code;
      const code = typeof raw === 'string' ? raw : undefined;
      const fallback = e instanceof Error ? e.message : String(e);
      // Expirado, respondido em outra tela ou apagado não volta: o cartão sai e o motivo vai no aviso.
      if (draftRefusalEndsCard(code)) {
        answered.add(draft.id);
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

  const source = SOURCE[draft.chat_name_source]?.();
  const file = draft.file;
  return <>
    <MachineGroup>
      <Block label={m.whatsapp_draft_to()}>
        <Text selectable style={[styles.number, { color: c.text }]}>{draft.chat_number}</Text>
        {draft.chat_name || source ? <Text style={[styles.small, { color: c.muted }]}>{[draft.chat_name, source].filter(Boolean).join(' · ')}</Text> : null}
        {draft.chat_name_source === 'push' ? <Text style={[styles.small, { color: warn }]}>{m.whatsapp_draft_push_warning()}</Text> : null}
      </Block>
      <MachineRow title={m.whatsapp_draft_origin()} value={draft.origin} />
      <MachineRow title={m.whatsapp_draft_expires()} value={formatCountdown(draftSecondsLeft(draft.expires_at, now))} />
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
            {markInvisibles(draft.text).map((p, i) => p.kind === 'mark'
              ? <Text key={i} style={{ color: warn, backgroundColor: withAlpha(warn, 0.14) }}>{invisibleMark(p.code)}</Text>
              : p.text)}
          </Text>
        </ScrollView>
      </Block>
    </MachineGroup> : null}
    {file ? <MachineGroup>
      <MachineRow title={m.whatsapp_draft_attachment()} value={`${file.name} · ${draftFileSize(file.size)}`} />
      <Block label={m.whatsapp_draft_path()}>
        <Text selectable style={[styles.mono, { color: file.outside_cwd ? warn : c.muted }]}>{file.source}</Text>
        {file.outside_cwd ? <Text style={[styles.small, { color: warn }]}>{m.whatsapp_draft_outside_cwd()}</Text> : null}
      </Block>
    </MachineGroup> : null}
    <Notice text={error} error />
    <MachineGroup>
      <MachineRow title={m.whatsapp_draft_send()} action disabled={busy} onPress={() => void answer('send')} />
      <MachineRow title={m.whatsapp_draft_decline()} danger disabled={busy} onPress={() => void answer('decline')} />
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
  button: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[0.5] },
  count: { fontSize: theme.base.text.xs },
  block: { paddingHorizontal: theme.base.space[4], paddingVertical: theme.base.space[3], gap: theme.base.space[1], borderTopWidth: 1 },
  small: { fontSize: theme.base.text.xs, lineHeight: 19 },
  number: { fontFamily: MONO, fontSize: theme.base.text.base, fontWeight: theme.base.weight.semibold },
  line: { fontSize: theme.base.text.sm },
  mono: { fontFamily: MONO, fontSize: theme.base.text.sm },
  textBox: { maxHeight: 150 },
}));
