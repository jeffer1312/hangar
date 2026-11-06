import { useEffect, useState } from 'react';
import { Pressable, Text, View, type StyleProp, type ViewStyle } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { answerVaultRequest, type Server, type ServerBucket, type VaultAnswer, type VaultEvent, type VaultPolicy, type VaultRequest } from '@hangar/core';
import { MachineField, MachineGroup, MachineRow } from '../machines/MachineRows';
import { MachineSheet, Notice } from '../machines/parts';
import { InfoNotice } from '../config/InfoNotice';
import { PolicyRows } from './VaultEntrySheet';
import { VaultApprovalRows } from './VaultApprovalRows';
import { openVaultRequest, useVaultTarget } from './target';
import { vaultErrorText } from './errors';
import { useSessions } from '../../stores/sessions';
import { useIdentities } from '../pair/identities';
import { Icon } from '../../ui/Icon';
import { toast } from '../../ui/Toast';
import { haptic } from '../../ui/haptics';
import { superficie } from '../../theme/superficie';
import * as m from '../../paraglide/messages';

// Respondido aqui some da folha na hora; o evento `vault` seguinte confirma.
const answered = new Set<string>();
const pending = (requests: VaultRequest[]) => requests.filter((r) => !answered.has(r.id));
// Estado de teste montado sem o campo `vault` não derruba a lista.
const NONE: Record<string, VaultEvent> = {};

export function RequestNotice({ title, onPress }: { title: string; onPress: () => void }) {
  const { theme } = useUnistyles();
  return <Pressable accessibilityRole="button" accessibilityLabel={title} accessibilityHint={m.vault_app_requests_open()}
    onPress={() => { haptic.selection(); onPress(); }}
    style={({ pressed }) => [styles.notice, { backgroundColor: superficie(theme, 0.6) }, pressed && { opacity: 0.7 }]}>
    <Icon name="KeyRound" size={18} color={theme.tokens.status.warning} />
    <View style={styles.noticeText}>
      <Text style={[styles.title, { color: theme.tokens.text.primary }]}>{title}</Text>
      <Text style={[styles.hint, { color: theme.tokens.text.muted }]}>{m.vault_app_requests_open()}</Text>
    </View>
    <Icon name="ChevronRight" size={17} color={theme.tokens.text.muted} />
  </Pressable>;
}

/** Aviso por máquina com pedido do cofre aberto (a máquina do aviso é a dona). */
export function VaultRequests({ byServer }: { byServer: ServerBucket[] }) {
  const vault = useSessions((s) => s.vault) ?? NONE;
  const live = byServer.filter((b) => !b.error && b.loaded);
  const notices = live.flatMap((b) => {
    const open = pending(vault[b.server.id]?.requests ?? []);
    return open.length ? [{ server: b.server, label: b.label ?? b.server.label, count: open.length }] : [];
  });
  return <>
    {notices.map(({ server, label, count }) => <RequestNotice key={server.id} title={m.vault_app_requests_title({ label, count })}
      onPress={() => openVaultRequest({ serverId: server.id, requestId: null })} />)}
  </>;
}

/** Botão de chave com a contagem, para barras sem espaço para o aviso (as três formas da barra do tablet). */
export function VaultRequestsButton({ style }: { style?: StyleProp<ViewStyle> }) {
  const { theme } = useUnistyles();
  const vault = useSessions((s) => s.vault) ?? NONE;
  const byServer = useSessions((s) => s.byServer) ?? [];
  const open = byServer.filter((b) => !b.error && b.loaded)
    .map((b) => ({ server: b.server, label: b.label ?? b.server.label, count: pending(vault[b.server.id]?.requests ?? []).length }))
    .filter((b) => b.count > 0);
  if (!open.length) return null;
  // A folha mostra uma máquina por vez: abre a que mais espera, e o rótulo fala dela.
  const first = open.reduce((a, b) => (b.count > a.count ? b : a));
  const count = open.reduce((n, b) => n + b.count, 0);
  return <Pressable accessibilityRole="button" accessibilityLabel={m.vault_app_requests_title({ label: first.label, count: first.count })}
    accessibilityHint={m.vault_app_requests_open()} style={[style, styles.button]}
    onPress={() => { haptic.selection(); openVaultRequest({ serverId: first.server.id, requestId: null }); }}>
    <Icon name="KeyRound" size={20} color={theme.tokens.status.warning} />
    <Text style={[styles.hint, { color: theme.tokens.status.warning }]}>{count}</Text>
  </Pressable>;
}

/** Pedido feito pela sessão aberta: aparece no chat dela, pela origem `servidor::sessao`. */
export function SessionVaultRequests({ serverId, name }: { serverId: string; name: string }) {
  const vault = useSessions((s) => s.vault) ?? NONE;
  const servers = useSessions((s) => s.servers) ?? [];
  const id = useIdentities(servers).identities.get(serverId);
  // Sem o identificador da máquina (ainda lendo ou falhou), vale o nome da sessão: o pedido não pode sumir do chat.
  const fromHere = (origin: string) => id ? origin === `${id}::${name}` : origin.endsWith(`::${name}`);
  const mine = Object.entries(vault).flatMap(([owner, event]) => pending(event.requests).filter((r) => fromHere(r.origin)).map((r) => ({ owner, r })));
  return <>
    {mine.map(({ owner, r }) => <RequestNotice key={r.id}
      title={r.kind === 'entry' ? m.vault_app_session_entry({ label: r.label ?? '' }) : m.vault_app_session_approve({ entry: r.entry_name ?? '' })}
      onPress={() => openVaultRequest({ serverId: owner, requestId: r.id })} />)}
  </>;
}

/** A folha que responde, na raiz do app: abre da lista, do chat e do push. */
export function VaultRequestSheet() {
  const vault = useSessions((s) => s.vault) ?? NONE;
  const byServer = useSessions((s) => s.byServer) ?? [];
  const target = useVaultTarget((s) => s.target);
  if (!target) return null;
  const bucket = byServer.find((b) => b.server.id === target.serverId);
  return <RequestSheet server={bucket?.server ?? null} ready={!!bucket?.loaded && !bucket.error && !!vault[target.serverId]}
    requests={pending(vault[target.serverId]?.requests ?? [])} requestId={target.requestId} />;
}

function RequestSheet({ server, ready, requests, requestId }: {
  server: Server | null;
  ready: boolean;
  requests: VaultRequest[];
  requestId: string | null;
}) {
  const close = () => openVaultRequest(null);
  const request = requestId ? requests.find((r) => r.id === requestId) : requests[0];
  // Respondido o último pedido aberto pela lista, a folha fecha sozinha.
  useEffect(() => { if (ready && !requestId && !request) close(); }, [ready, requestId, request]);
  const next = () => { if (server) openVaultRequest({ serverId: server.id, requestId: null }); };

  return <MachineSheet title={m.vault_app_request_title()} onClose={close}>
    {!server ? <Notice text={m.vault_app_request_no_server()} error />
      : !ready ? <InfoNotice text={m.vault_app_loading()} />
      : !request ? <>
        <InfoNotice text={m.vault_err_pedido_perdido()} />
        {requests.length ? <MachineGroup><MachineRow title={m.vault_app_request_next()} action onPress={next} /></MachineGroup> : null}
      </>
      : <RequestCard key={request.id} server={server} request={request} onDone={next} />}
  </MachineSheet>;
}

function RequestCard({ server, request, onDone }: { server: Server; request: VaultRequest; onDone: () => void }) {
  const [password, setPassword] = useState('');
  const [name, setName] = useState(request.label ?? '');
  const [policy, setPolicy] = useState<VaultPolicy>('ask');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');

  const send = async (answer: VaultAnswer, done: string) => {
    if (busy) return;
    setBusy(true);
    setError('');
    try {
      await answerVaultRequest(server, request.id, answer);
      setPassword('');
      answered.add(request.id);
      toast.ok(done);
      onDone();
    } catch (e) {
      const code = (e as { code?: unknown } | null)?.code;
      // Pedido que outra tela respondeu ou que expirou não volta: o cartão sai e o motivo vai no aviso.
      if (code === 'erro_interacao_desatualizada' || code === 'erro_cofre_pedido_respondido' || code === 'erro_cofre_pedido_perdido') {
        answered.add(request.id);
        toast.erro(vaultErrorText(e));
        onDone();
        return;
      }
      setError(vaultErrorText(e));
      haptic.error();
    } finally { setBusy(false); }
  };
  const iid = request.interaction_id;

  if (request.kind === 'approve') return <VaultApprovalRows request={request} busy={busy} notice={<Notice text={error} error />}
    onAllow={() => void send({ interaction_id: iid, decision: 'allow' }, m.vault_app_allowed())}
    onDeny={() => void send({ interaction_id: iid, decision: 'deny' }, m.vault_app_denied())} />;

  const save = (keep: boolean) => {
    if (!password) return;
    void send({ interaction_id: iid, decision: 'save', password, keep, name: keep ? name.trim() || undefined : undefined, policy: keep ? policy : undefined },
      keep ? m.vault_app_saved() : m.vault_app_sent_once());
  };
  return <>
    <MachineGroup>
      <MachineRow title={request.label ?? m.vault_app_request_entry()} description={request.reason} />
      <MachineRow title={m.vault_app_request_origin()} value={request.origin} />
    </MachineGroup>
    <MachineGroup>
      <MachineField label={m.vault_app_field_password()} value={password} onChangeText={setPassword} editable={!busy}
        secureTextEntry autoComplete="off" textContentType="none" placeholder={request.placeholder} />
      <MachineField label={m.vault_app_field_name()} value={name} onChangeText={setName} editable={!busy} />
    </MachineGroup>
    <MachineGroup>
      <PolicyRows value={policy} onChange={setPolicy} disabled={busy} />
    </MachineGroup>
    <Notice text={error} error />
    <MachineGroup>
      <MachineRow title={m.vault_app_keep()} action disabled={busy || !password} onPress={() => save(true)} />
      <MachineRow title={m.vault_app_once()} disabled={busy || !password} onPress={() => save(false)} />
      <MachineRow title={m.vault_app_decline()} danger disabled={busy} onPress={() => void send({ interaction_id: iid, decision: 'decline' }, m.vault_app_declined())} />
    </MachineGroup>
  </>;
}

const styles = StyleSheet.create((theme) => ({
  notice: {
    minHeight: 56, flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2],
    marginHorizontal: theme.base.space[3], marginVertical: theme.base.space[1], paddingHorizontal: theme.base.space[3], paddingVertical: theme.base.space[2],
    borderRadius: theme.base.radius.lg,
  },
  noticeText: { flex: 1, gap: theme.base.space[0.5] },
  title: { fontSize: theme.base.text.sm, fontWeight: theme.base.weight.semibold },
  hint: { fontSize: theme.base.text.xs },
  button: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[0.5] },
}));
