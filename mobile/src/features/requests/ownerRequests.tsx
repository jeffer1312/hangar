import { useEffect, useMemo, type ComponentType } from 'react';
import { Pressable, Text, View, type StyleProp, type ViewStyle } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import type { Server, ServerBucket } from '@hangar/core';
import { MachineGroup, MachineRow } from '../machines/MachineRows';
import { MachineSheet, Notice } from '../machines/parts';
import { InfoNotice } from '../config/InfoNotice';
import { useSessions, type SessionsState } from '../../stores/sessions';
import { useIdentities } from '../pair/identities';
import { Icon, type IconName } from '../../ui/Icon';
import { haptic } from '../../ui/haptics';
import { superficie } from '../../theme/superficie';

/** Pedido que espera o dono; `origin` é `servidor::sessao` de quem pediu. */
export interface OwnerRequest { id: string; origin: string }

/** O pedido que a folha mostra; `id: null` = o primeiro da máquina. */
export interface RequestTarget { serverId: string; id: string | null }

/** O que cada tipo de pedido (cofre, rascunho de WhatsApp) entrega à camada comum. */
export interface OwnerRequestsSpec<E, T extends OwnerRequest> {
  /** Último evento de cada máquina, pelo id dela. */
  select: (s: SessionsState) => Record<string, E> | undefined;
  items: (event: E) => T[];
  useTarget: () => RequestTarget | null;
  setTarget: (target: RequestTarget | null) => void;
  icon: IconName;
  hint: () => string;
  /** Aviso da máquina na lista e rótulo do botão da barra. */
  machineTitle: (p: { label: string; count: number }) => string;
  /** Aviso no chat da sessão que pediu. */
  sessionTitle: (item: T) => string;
  sheet: { title: () => string; noServer: () => string; loading: () => string; gone: () => string; next: () => string };
  /** O corpo da folha para um pedido; `onDone` abre o próximo da máquina. */
  Card: ComponentType<{ server: Server; item: T; onDone: () => void }>;
}

/** Máquinas com a lista de pé: pedido de máquina caída não aparece (a lista nova traz de volta). */
const liveServers = (byServer: ServerBucket[]) => byServer.filter((b) => !b.error && b.loaded);

/** Pedidos esperando o dono, por máquina: aviso na lista, botão da barra, aviso no chat de quem pediu e a folha que responde. */
export function ownerRequests<E, T extends OwnerRequest>(spec: OwnerRequestsSpec<E, T>) {
  // Respondido aqui some da folha na hora; o evento seguinte da máquina confirma.
  const answered = new Set<string>();
  const pending = (event: E | undefined) => (event ? spec.items(event) : []).filter((r) => !answered.has(r.id));
  // Estado de teste montado sem o campo não derruba a lista.
  const NONE: Record<string, E> = {};
  const useEvents = () => useSessions(spec.select) ?? NONE;
  const waiting = (byServer: ServerBucket[], events: Record<string, E>) => liveServers(byServer).flatMap((b) => {
    const count = pending(events[b.server.id]).length;
    return count ? [{ server: b.server, label: b.label ?? b.server.label, count }] : [];
  });

  /** Aviso por máquina com pedido esperando (a máquina do aviso é a que responde). */
  function Notices({ byServer }: { byServer: ServerBucket[] }) {
    const events = useEvents();
    return <>
      {waiting(byServer, events).map(({ server, label, count }) => <RequestNotice key={server.id} icon={spec.icon} hint={spec.hint()}
        title={spec.machineTitle({ label, count })} onPress={() => spec.setTarget({ serverId: server.id, id: null })} />)}
    </>;
  }

  /** Botão com a contagem, para barras sem espaço para o aviso. */
  function CountButton({ style }: { style?: StyleProp<ViewStyle> }) {
    const { theme } = useUnistyles();
    const events = useEvents();
    const byServer = useSessions((s) => s.byServer) ?? [];
    const open = waiting(byServer, events);
    if (!open.length) return null;
    // A folha mostra uma máquina por vez: abre a que mais espera, e o rótulo fala dela.
    const first = open.reduce((a, b) => (b.count > a.count ? b : a));
    const count = open.reduce((n, b) => n + b.count, 0);
    return <Pressable accessibilityRole="button" accessibilityLabel={spec.machineTitle({ label: first.label, count: first.count })}
      accessibilityHint={spec.hint()} style={[style, styles.button]}
      onPress={() => { haptic.selection(); spec.setTarget({ serverId: first.server.id, id: null }); }}>
      <Icon name={spec.icon} size={20} color={theme.tokens.status.warning} />
      <Text style={[styles.hint, { color: theme.tokens.status.warning }]}>{count}</Text>
    </Pressable>;
  }

  /** Pedido feito pela sessão aberta: aparece no chat dela, pela origem `servidor::sessao`. */
  function SessionNotices({ serverId, name }: { serverId: string; name: string }) {
    const events = useEvents();
    const servers = useSessions((s) => s.servers) ?? [];
    // Só os ids das máquinas de pé, juntos num texto: a lista, que muda a cada evento, não redesenha o aviso.
    const liveIds = useSessions((s) => liveServers(s.byServer ?? []).map((b) => b.server.id).join('\n'));
    const live = useMemo(() => new Set(liveIds.split('\n')), [liveIds]);
    const id = useIdentities(servers).identities.get(serverId);
    // Sem o identificador da máquina (ainda lendo ou falhou), vale o nome da sessão: o pedido não pode sumir do chat.
    const fromHere = (origin: string) => id ? origin === `${id}::${name}` : origin.endsWith(`::${name}`);
    const mine = Object.entries(events).filter(([owner]) => live.has(owner))
      .flatMap(([owner, event]) => pending(event).filter((r) => fromHere(r.origin)).map((item) => ({ owner, item })));
    return <>
      {mine.map(({ owner, item }) => <RequestNotice key={item.id} icon={spec.icon} hint={spec.hint()} title={spec.sessionTitle(item)}
        onPress={() => spec.setTarget({ serverId: owner, id: item.id })} />)}
    </>;
  }

  /** A folha que responde, na raiz do app: abre da lista, do chat e do push. */
  function Sheet() {
    const events = useEvents();
    const byServer = useSessions((s) => s.byServer) ?? [];
    const target = spec.useTarget();
    if (!target) return null;
    const bucket = byServer.find((b) => b.server.id === target.serverId);
    return <SheetBody server={bucket?.server ?? null} ready={!!bucket?.loaded && !bucket.error && !!events[target.serverId]}
      items={pending(events[target.serverId])} id={target.id} />;
  }

  function SheetBody({ server, ready, items, id }: { server: Server | null; ready: boolean; items: T[]; id: string | null }) {
    const close = () => spec.setTarget(null);
    const item = id ? items.find((r) => r.id === id) : items[0];
    // Respondido o último pedido aberto pela lista, a folha fecha sozinha.
    useEffect(() => { if (ready && !id && !item) close(); }, [ready, id, item]);
    const next = () => { if (server) spec.setTarget({ serverId: server.id, id: null }); };
    const { Card } = spec;
    return <MachineSheet title={spec.sheet.title()} onClose={close}>
      {!server ? <Notice text={spec.sheet.noServer()} error />
        : !ready ? <InfoNotice text={spec.sheet.loading()} />
        : !item ? <>
          <InfoNotice text={spec.sheet.gone()} />
          {items.length ? <MachineGroup><MachineRow title={spec.sheet.next()} action onPress={next} /></MachineGroup> : null}
        </>
        : <Card key={item.id} server={server} item={item} onDone={next} />}
    </MachineSheet>;
  }

  return { answered, Notices, CountButton, SessionNotices, Sheet };
}

/** Aviso de pedido esperando resposta, na lista e no chat. */
export function RequestNotice({ title, onPress, icon, hint }: { title: string; onPress: () => void; icon: IconName; hint: string }) {
  const { theme } = useUnistyles();
  return <Pressable accessibilityRole="button" accessibilityLabel={title} accessibilityHint={hint}
    onPress={() => { haptic.selection(); onPress(); }}
    style={({ pressed }) => [styles.notice, { backgroundColor: superficie(theme, 0.6) }, pressed && { opacity: 0.7 }]}>
    <Icon name={icon} size={18} color={theme.tokens.status.warning} />
    <View style={styles.noticeText}>
      <Text style={[styles.title, { color: theme.tokens.text.primary }]}>{title}</Text>
      <Text style={[styles.hint, { color: theme.tokens.text.muted }]}>{hint}</Text>
    </View>
    <Icon name="ChevronRight" size={17} color={theme.tokens.text.muted} />
  </Pressable>;
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
