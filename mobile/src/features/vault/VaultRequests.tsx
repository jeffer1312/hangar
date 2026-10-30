import { useEffect, useState } from 'react';
import { Pressable, Text, View } from 'react-native';
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

/** Aviso por máquina com pedido do cofre aberto, e a folha que responde (a máquina do aviso é a dona). */
export function VaultRequests({ byServer }: { byServer: ServerBucket[] }) {
  const { theme } = useUnistyles();
  const vault = useSessions((s) => s.vault) ?? NONE;
  const target = useVaultTarget((s) => s.target);
  const live = byServer.filter((b) => !b.error && b.loaded);
  const notices = live.flatMap((b) => {
    const open = pending(vault[b.server.id]?.requests ?? []);
    return open.length ? [{ server: b.server, label: b.label ?? b.server.label, count: open.length }] : [];
  });
  const sheetBucket = target ? byServer.find((b) => b.server.id === target.serverId) : undefined;

  return <>
    {notices.map(({ server, label, count }) => <Pressable key={server.id} accessibilityRole="button"
      accessibilityLabel={m.vault_app_requests_title({ label, count })} accessibilityHint={m.vault_app_requests_open()}
      onPress={() => { haptic.selection(); openVaultRequest({ serverId: server.id, requestId: null }); }}
      style={({ pressed }) => [styles.notice, { backgroundColor: superficie(theme, 0.6) }, pressed && { opacity: 0.7 }]}>
      <Icon name="KeyRound" size={18} color={theme.tokens.status.warning} />
      <View style={styles.noticeText}>
        <Text style={[styles.title, { color: theme.tokens.text.primary }]}>{m.vault_app_requests_title({ label, count })}</Text>
        <Text style={[styles.hint, { color: theme.tokens.text.muted }]}>{m.vault_app_requests_open()}</Text>
      </View>
      <Icon name="ChevronRight" size={17} color={theme.tokens.text.muted} />
    </Pressable>)}
    {target ? <RequestSheet server={sheetBucket?.server ?? null} ready={!!sheetBucket?.loaded && !sheetBucket.error && !!vault[target.serverId]}
      requests={pending(vault[target.serverId]?.requests ?? [])} requestId={target.requestId} /> : null}
  </>;
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
      // Pedido que outra tela respondeu ou que expirou não volta: sai daqui também.
      if (code === 'erro_interacao_desatualizada' || code === 'erro_cofre_pedido_respondido' || code === 'erro_cofre_pedido_perdido') answered.add(request.id);
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
  noticeText: { flex: 1, gap: 2 },
  title: { fontSize: theme.base.text.sm, fontWeight: theme.base.weight.semibold },
  hint: { fontSize: theme.base.text.xs },
}));
