import { useEffect, useRef, useState } from 'react';
import { Pressable, Text, View, type StyleProp, type ViewStyle } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import {
  answerIntegrationApproval,
  ownServers,
  type IntegrationApprovalDecision,
  type IntegrationApprovalEvent,
  type IntegrationApprovalRequest,
  type Server,
  type ServerBucket,
} from '@hangar/core';
import { MachineGroup, MachineRow } from '../machines/MachineRows';
import { MachineSheet, Notice } from '../machines/parts';
import { InfoNotice } from '../config/InfoNotice';
import { integrationErrorText } from './errors';
import { openIntegrationApproval, useIntegrationApprovalTarget } from './target';
import { useSessions } from '../../stores/sessions';
import { useIdentities } from '../pair/identities';
import { Icon } from '../../ui/Icon';
import { toast } from '../../ui/Toast';
import { haptic } from '../../ui/haptics';
import { superficie } from '../../theme/superficie';
import * as m from '../../paraglide/messages';

// Pedido respondido sai da folha na hora; o próximo evento `integrations_approvals` confirma.
const answered = new Set<string>();
const pending = (requests: IntegrationApprovalRequest[]) => requests.filter((r) => !answered.has(r.id));
// Estado de teste montado sem o campo `integrationApprovals` não derruba a lista.
const NONE: Record<string, IntegrationApprovalEvent> = {};
const EMPTY_BUCKETS: ServerBucket[] = [];
const EMPTY_SERVERS: Server[] = [];

const GONE_CODES = new Set(['approval_stale', 'approval_answered', 'approval_lost', 'approval_not_found', 'approval_conflict']);

function ApprovalNotice({ title, onPress }: { title: string; onPress: () => void }) {
  const { theme } = useUnistyles();
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={title}
      accessibilityHint={m.integrations_approval_requests_open()}
      onPress={() => { haptic.selection(); onPress(); }}
      style={({ pressed }) => [styles.notice, { backgroundColor: superficie(theme, 0.6) }, pressed && { opacity: 0.7 }]}
    >
      <Icon name="Plug" size={18} color={theme.tokens.status.warning} />
      <View style={styles.noticeText}>
        <Text style={[styles.title, { color: theme.tokens.text.primary }]}>{title}</Text>
        <Text style={[styles.hint, { color: theme.tokens.text.muted }]}>{m.integrations_approval_requests_open()}</Text>
      </View>
      <Icon name="ChevronRight" size={17} color={theme.tokens.text.muted} />
    </Pressable>
  );
}

/** Aviso por máquina com pedido de aprovação de integração aberto. */
export function IntegrationRequests({ byServer }: { byServer: ServerBucket[] }) {
  const approvals = useSessions((s) => s.integrationApprovals) ?? NONE;
  const live = byServer.filter((b) => !b.error && b.loaded);
  const notices = live.flatMap((b) => {
    const open = pending(approvals[b.server.id]?.requests ?? []);
    return open.length ? [{ server: b.server, label: b.label ?? b.server.label, count: open.length }] : [];
  });
  return (
    <>
      {notices.map(({ server, label, count }) => (
        <ApprovalNotice
          key={server.id}
          title={m.integrations_approval_requests_title({ label, count })}
          onPress={() => openIntegrationApproval({ serverId: server.id, requestId: null })}
        />
      ))}
    </>
  );
}

/** Botão compacto com contagem, para a barra lateral/horizontal do tablet. */
export function IntegrationRequestsButton({ style }: { style?: StyleProp<ViewStyle> }) {
  const { theme } = useUnistyles();
  const approvals = useSessions((s) => s.integrationApprovals) ?? NONE;
  const byServer = useSessions((s) => s.byServer) ?? EMPTY_BUCKETS;
  const open = byServer
    .filter((b) => !b.error && b.loaded)
    .map((b) => ({ server: b.server, label: b.label ?? b.server.label, count: pending(approvals[b.server.id]?.requests ?? []).length }))
    .filter((b) => b.count > 0);
  if (!open.length) return null;
  const first = open.reduce((a, b) => (b.count > a.count ? b : a));
  const count = open.reduce((n, b) => n + b.count, 0);
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={m.integrations_approval_requests_title({ label: first.label, count: first.count })}
      accessibilityHint={m.integrations_approval_requests_open()}
      style={[style, styles.button]}
      onPress={() => { haptic.selection(); openIntegrationApproval({ serverId: first.server.id, requestId: null }); }}
    >
      <Icon name="Plug" size={20} color={theme.tokens.status.warning} />
      <Text style={[styles.hint, { color: theme.tokens.status.warning }]}>{count}</Text>
    </Pressable>
  );
}

/** Aviso no chat da sessão que pediu a ferramenta externa. */
export function SessionIntegrationRequests({ serverId, name }: { serverId: string; name: string }) {
  const approvals = useSessions((s) => s.integrationApprovals) ?? NONE;
  const servers = useSessions((s) => s.servers) ?? EMPTY_SERVERS;
  const id = useIdentities(servers).identities.get(serverId);
  const singleOwnServer = ownServers(servers).length <= 1;
  const fromHere = (owner: string, origin: string) => {
    if (id && origin === `${id}::${name}`) return true;
    // Enquanto a identidade da máquina não resolveu, só casa por sufixo se for a dona única.
    if (!id && owner === serverId && singleOwnServer && origin.endsWith(`::${name}`)) return true;
    return !origin.includes('::') && owner === serverId && origin === name;
  };
  const mine = Object.entries(approvals).flatMap(([owner, event]) =>
    pending(event.requests).filter((r) => fromHere(owner, r.origin)).map((r) => ({ owner, r })),
  );
  return (
    <>
      {mine.map(({ owner, r }) => (
        <ApprovalNotice
          key={r.id}
          title={m.integrations_approval_session_notice({ integration: r.integration_name || r.integration_id, tool: r.tool })}
          onPress={() => openIntegrationApproval({ serverId: owner, requestId: r.id })}
        />
      ))}
    </>
  );
}

/** Folha global de aprovação de ferramenta externa, montada na raiz do app. */
export function IntegrationRequestSheet() {
  const approvals = useSessions((s) => s.integrationApprovals) ?? NONE;
  const byServer = useSessions((s) => s.byServer) ?? EMPTY_BUCKETS;
  const target = useIntegrationApprovalTarget((s) => s.target);
  if (!target) return null;
  const bucket = byServer.find((b) => b.server.id === target.serverId);
  return (
    <ApprovalSheet
      server={bucket?.server ?? null}
      ready={!!bucket?.loaded && !bucket.error && !!approvals[target.serverId]}
      requests={pending(approvals[target.serverId]?.requests ?? [])}
      requestId={target.requestId}
    />
  );
}

function ApprovalSheet({ server, ready, requests, requestId }: {
  server: Server | null;
  ready: boolean;
  requests: IntegrationApprovalRequest[];
  requestId: string | null;
}) {
  const close = () => openIntegrationApproval(null);
  const request = requestId ? requests.find((r) => r.id === requestId) : requests[0];
  useEffect(() => {
    if (ready && !requestId && !request) close();
  }, [ready, requestId, request]);
  const next = () => {
    if (!server) return;
    const current = useIntegrationApprovalTarget.getState().target;
    // Só avança se a folha ainda estiver na máquina e no pedido desta operação.
    if (!current || current.serverId !== server.id || current.requestId !== requestId) return;
    openIntegrationApproval({ serverId: server.id, requestId: null });
  };

  return (
    <MachineSheet title={m.integrations_approval_title()} onClose={close}>
      {!server ? (
        <Notice text={m.integrations_approval_no_server()} error />
      ) : !ready ? (
        <InfoNotice text={m.integrations_loading()} />
      ) : !request ? (
        <>
          <InfoNotice text={m.integrations_approval_lost()} />
          {requests.length ? (
            <MachineGroup>
              <MachineRow title={m.integrations_approval_next()} action onPress={next} />
            </MachineGroup>
          ) : null}
        </>
      ) : (
        <ApprovalCard key={request.id} server={server} request={request} onDone={next} />
      )}
    </MachineSheet>
  );
}

function ApprovalCard({ server, request, onDone }: {
  server: Server;
  request: IntegrationApprovalRequest;
  onDone: () => void;
}) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const inFlight = useRef(false);

  const send = async (decision: IntegrationApprovalDecision, doneText: string) => {
    if (inFlight.current) return;
    inFlight.current = true;
    setBusy(true);
    setError('');
    try {
      await answerIntegrationApproval(server, request.id, {
        interaction_id: request.interaction_id,
        decision,
      });
      answered.add(request.id);
      toast.ok(doneText);
      onDone();
    } catch (e) {
      const code = (e as { code?: unknown } | null)?.code;
      if (typeof code === 'string' && GONE_CODES.has(code)) {
        answered.add(request.id);
        toast.erro(integrationErrorText(e));
        onDone();
        return;
      }
      setError(integrationErrorText(e));
      haptic.error();
    } finally {
      inFlight.current = false;
      setBusy(false);
    }
  };

  return (
    <>
      <MachineGroup>
        <MachineRow title={m.integrations_approval_integration()} value={request.integration_name || request.integration_id} />
        <MachineRow title={m.integrations_approval_tool()} value={request.tool} />
        <MachineRow title={m.integrations_approval_origin()} value={request.origin} />
        {request.reason ? <MachineRow title={m.integrations_approval_reason()} description={request.reason} /> : null}
        {request.arguments_preview ? <MachineRow title={m.integrations_approval_args()} description={request.arguments_preview} /> : null}
      </MachineGroup>
      <Notice text={error} error />
      <MachineGroup>
        <MachineRow
          title={m.integrations_approval_allow_once()}
          action
          disabled={busy}
          onPress={() => void send('allow_once', m.integrations_approval_allowed_once())}
        />
        <MachineRow
          title={m.integrations_approval_allow_always()}
          action
          disabled={busy}
          onPress={() => void send('allow_always', m.integrations_approval_allowed_always())}
        />
        <MachineRow
          title={m.integrations_approval_deny()}
          danger
          disabled={busy}
          onPress={() => void send('deny', m.integrations_approval_denied())}
        />
      </MachineGroup>
    </>
  );
}

const styles = StyleSheet.create((theme) => ({
  notice: {
    minHeight: 56,
    flexDirection: 'row',
    alignItems: 'center',
    gap: theme.base.space[2],
    marginHorizontal: theme.base.space[3],
    marginVertical: theme.base.space[1],
    paddingHorizontal: theme.base.space[3],
    paddingVertical: theme.base.space[2],
    borderRadius: theme.base.radius.lg,
  },
  noticeText: { flex: 1, gap: theme.base.space[0.5] },
  title: { fontSize: theme.base.text.sm, fontWeight: theme.base.weight.semibold },
  hint: { fontSize: theme.base.text.xs },
  button: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[0.5] },
}));
