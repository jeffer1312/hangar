import { Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { relativeTime } from '@hangar/core';
import type { PeerMsg } from './pairFeed';
import * as m from '../../paraglide/messages';

interface Props {
  sessionName: string;
  feed: PeerMsg[];
  failed: string[];
  loading: boolean;
}

export function PairFeed({ sessionName, feed, failed, loading }: Props) {
  const { theme } = useUnistyles();

  return (
    <View style={[styles.wrap, { borderTopColor: theme.tokens.border.subtle }]}>
      <Text style={[styles.title, { color: theme.tokens.text.secondary }]}>{m.par_conversa_titulo()}</Text>
      {failed.length ? <Text style={[styles.warning, { color: theme.tokens.status.warning }]} accessibilityRole="alert" selectable>{`⚠ ${m.par_sem_historico({ nomes: failed.join(', ') })}`}</Text> : null}
      {loading ? (
        <Text style={[styles.empty, { color: theme.tokens.text.muted }]}>{m.comum_carregando()}</Text>
      ) : feed.length === 0 ? (
        <Text style={[styles.empty, { color: theme.tokens.text.muted }]}>{m.par_vazio_trocas()}</Text>
      ) : (
        <View style={styles.list}>
          {feed.map((message, index) => (
            <View key={`${message.ts}-${message.from}-${message.to}-${index}`} style={[styles.item, { backgroundColor: theme.tokens.bg.surface, borderColor: message.from === sessionName ? theme.tokens.accent.base : theme.tokens.border.subtle }]}>
              <Text style={[styles.meta, { color: theme.tokens.text.muted }]} selectable>
                {message.from} → {message.to}{message.ts ? ` · ${relativeTime(message.ts)}` : ''}
              </Text>
              <Text style={[styles.text, { color: theme.tokens.text.primary }]} numberOfLines={4} selectable>{message.text}</Text>
            </View>
          ))}
        </View>
      )}
    </View>
  );
}

const styles = StyleSheet.create((theme) => ({
  wrap: { gap: theme.base.space[2], borderTopWidth: 1, paddingTop: theme.base.space[3] },
  title: { fontSize: theme.base.text.sm, fontWeight: '700' },
  warning: { fontSize: theme.base.text.xs, lineHeight: 18 },
  empty: { fontSize: theme.base.text.sm, textAlign: 'center', paddingVertical: theme.base.space[3] },
  list: { gap: theme.base.space[2] },
  item: { gap: 3, paddingHorizontal: theme.base.space[3], paddingVertical: theme.base.space[2], borderWidth: 1, borderRadius: theme.base.radius.md },
  meta: { fontSize: theme.base.text.xs },
  text: { fontSize: theme.base.text.sm, lineHeight: 20 },
}));
