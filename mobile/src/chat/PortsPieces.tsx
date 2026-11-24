// Peças visuais das portas, sem loja nem rede: a folha, a tela da página e a animação da novidade
// usam as mesmas, e a animação roda em teste sem os módulos nativos que as lojas carregam.
import type { ReactNode } from 'react';
import { ActivityIndicator, Pressable, ScrollView, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import type { PortEntry } from '@hangar/core';
import { Icon } from '../ui/Icon';
import { INK } from '../theme/scale';
import * as m from '../paraglide/messages';

/** Uma porta da folha. `busy` é a porta que está abrindo; `onStop` mostra o "Parar servidor", e
 * `stopping` é a porta que está parando. `onLink` (com o túnel) mostra o "Link de fora" e faz o toque
 * abrir no app. */
export function PortRow({ port: p, busy, failure, onPress, onStop, stopping = null, onLink }: {
  port: PortEntry; busy: number | null; failure: string | null; onPress: () => void; onStop?: () => void; stopping?: number | null;
  onLink?: () => void;
}) {
  const { theme } = useUnistyles();
  const label = p.process ? `${p.port} · ${p.process}` : String(p.port);
  const opening = busy === p.port;
  const inApp = !!onLink;
  return (
    <View>
      <View style={styles.line}>
        <Pressable
          onPress={onPress}
          disabled={busy !== null}
          style={[styles.row, busy !== null && !opening ? { opacity: 0.5 } : null]}
          accessibilityRole="button"
          accessibilityLabel={inApp ? m.mobile_ports_open_app_label({ label }) : m.mobile_ports_open_label({ label })}
          accessibilityState={{ busy: opening, disabled: busy !== null }}
        >
          <Text style={[styles.port, { color: theme.tokens.text.primary }]} numberOfLines={1}>{label}</Text>
          {opening && !inApp ? <ActivityIndicator color={theme.tokens.text.muted} />
            : <Icon name={inApp ? 'ChevronRight' : 'ExternalLink'} size={16} color={theme.tokens.text.muted} />}
        </Pressable>
        {onLink ? (
          <Pressable
            onPress={onLink}
            disabled={busy !== null}
            style={[styles.stop, busy !== null && !opening ? { opacity: 0.5 } : null]}
            accessibilityRole="button"
            accessibilityLabel={m.mobile_ports_link_label({ label })}
            accessibilityState={{ busy: opening, disabled: busy !== null }}
          >
            {opening ? <ActivityIndicator color={theme.tokens.text.muted} /> : <Icon name="Link" size={16} color={theme.tokens.text.muted} />}
          </Pressable>
        ) : null}
        {onStop ? (
          <Pressable
            onPress={onStop}
            disabled={stopping !== null}
            style={[styles.stop, stopping !== null && stopping !== p.port ? { opacity: 0.5 } : null]}
            accessibilityRole="button"
            accessibilityLabel={m.ports_stop_server_label({ label })}
            accessibilityState={{ busy: stopping === p.port, disabled: stopping !== null }}
          >
            {stopping === p.port ? <ActivityIndicator color={theme.tokens.text.muted} /> : <Icon name="Square" size={16} color={theme.tokens.status.warning} />}
          </Pressable>
        ) : null}
      </View>
      {p.shared ? (
        <Text style={[styles.error, { color: theme.tokens.text.muted }]}>{m.mobile_ports_shared()}</Text>
      ) : null}
      {failure ? (
        <Text style={[styles.error, { color: theme.tokens.status.warning }]} accessibilityRole="alert">{failure}</Text>
      ) : null}
    </View>
  );
}

/** Título, conteúdo e o aviso de como as portas abrem. */
export function PortsBody({ children, note }: { children: ReactNode; note?: string }) {
  const { theme } = useUnistyles();
  return (
    <View style={styles.inner}>
      <Text style={[styles.title, { color: theme.tokens.text.primary }]}>{m.mobile_ports_title()}</Text>
      {children}
      <Text style={[styles.note, { color: theme.tokens.text.muted }]}>{note ?? m.mobile_ports_one_preview()}</Text>
    </View>
  );
}

/** Uma aba por porta aberta no app, na tela da página. */
export function PortTabs({ tabs, active, onSelect }: { tabs: number[]; active: number; onSelect: (port: number) => void }) {
  const { theme } = useUnistyles();
  return (
    <ScrollView horizontal showsHorizontalScrollIndicator={false} contentContainerStyle={styles.tabs}
      accessibilityRole="tablist" accessibilityLabel={m.mobile_ports_tunnel_tabs()}>
      {tabs.map((p) => (
        <Pressable key={p} onPress={() => onSelect(p)} style={[styles.tab, p === active && { backgroundColor: theme.tokens.accent.base }]}
          accessibilityRole="tab" accessibilityState={{ selected: p === active }}>
          <Text style={[styles.tabText, { color: p === active ? INK.onAccent : theme.tokens.text.primary }]}>{p}</Text>
        </Pressable>
      ))}
    </ScrollView>
  );
}

/** Texto de estado da folha (vazio, carregando, erro). */
export function PortsStatus({ text, tone }: { text: string; tone?: 'warning' }) {
  const { theme } = useUnistyles();
  return <Text style={[styles.status, { color: tone ? theme.tokens.status.warning : theme.tokens.text.muted }]}
    accessibilityRole={tone ? 'alert' : undefined}>{text}</Text>;
}

/** Carregando, com o mesmo texto da folha. */
export function PortsLoading() {
  const { theme } = useUnistyles();
  return (
    <View style={styles.loading}>
      <ActivityIndicator color={theme.tokens.text.muted} />
      <Text style={[styles.status, { color: theme.tokens.text.muted }]}>{m.mobile_ports_loading()}</Text>
    </View>
  );
}

const styles = StyleSheet.create((theme) => ({
  inner: { gap: theme.base.space[1], padding: theme.base.space[3] },
  title: { fontSize: theme.base.text.base, fontWeight: theme.base.weight.semibold, marginBottom: theme.base.space[2] },
  loading: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2], minHeight: 44 },
  status: { fontSize: theme.base.text.sm, paddingVertical: theme.base.space[2] },
  line: { flexDirection: 'row', alignItems: 'center' },
  row: {
    flex: 1,
    flexDirection: 'row',
    alignItems: 'center',
    gap: theme.base.space[3],
    minHeight: 52,
    paddingHorizontal: theme.base.space[2],
    borderRadius: theme.base.radius.md,
  },
  stop: { width: 44, height: 44, alignItems: 'center', justifyContent: 'center', borderRadius: theme.base.radius.md },
  port: { flex: 1, fontSize: theme.base.text.base, fontWeight: theme.base.weight.semibold },
  error: { fontSize: theme.base.text.sm, paddingHorizontal: theme.base.space[2], paddingBottom: theme.base.space[2] },
  note: { fontSize: theme.base.text.xs, marginTop: theme.base.space[2] },
  tabs: { gap: theme.base.space[2], paddingHorizontal: theme.base.space[3], paddingVertical: theme.base.space[2] },
  tab: { minWidth: 56, minHeight: 44, alignItems: 'center', justifyContent: 'center', paddingHorizontal: theme.base.space[3], borderRadius: theme.base.radius.md },
  tabText: { fontSize: theme.base.text.sm, fontWeight: theme.base.weight.semibold, fontVariant: ['tabular-nums'] },
}));
