import { useCallback, useEffect, useState } from 'react';
import { Alert, InteractionManager, Text, View } from 'react-native';
import { StyleSheet } from 'react-native-unistyles';
import { fmtBytes } from '@hangar/core';
import { BlockHead, Box, Muted } from '../config/ServerConfigParts';
import { Pill } from '../config/PageHeader';
import { useSettingsColors, withAlpha } from '../config/colors';
import { haptic } from '../../ui/haptics';
import { toast } from '../../ui/Toast';
import * as m from '../../paraglide/messages';
import { cacheUsage, clearMediaCache, MEDIA_CACHE_LIMIT, type CacheKind, type CacheUsage } from './mediaCache';
import { VIDEO_CACHE_LIMIT } from './videoCache';

type Load = { status: 'loading' } | { status: 'error'; error: string } | { status: 'ready'; usage: CacheUsage };

const KINDS: { kind: CacheKind; label: () => string; alpha: number }[] = [
  { kind: 'image', label: m.media_cache_images, alpha: 1 },
  { kind: 'video', label: m.media_cache_videos, alpha: 0.7 },
  { kind: 'audio', label: m.media_cache_audio, alpha: 0.45 },
  { kind: 'document', label: m.media_cache_documents, alpha: 0.25 },
];

/** Espaço dos anexos baixados neste aparelho, por tipo, e o "Limpar" (inclui o cache de reprodução de vídeo). */
export function MediaCacheSection() {
  const c = useSettingsColors();
  const [load, setLoad] = useState<Load>({ status: 'loading' });
  const [clearing, setClearing] = useState(false);

  const read = useCallback(() => {
    // Lista a pasta inteira: depois da animação de entrada, para não travar a abertura da tela.
    const task = InteractionManager.runAfterInteractions(() => {
      cacheUsage()
        .then((usage) => setLoad({ status: 'ready', usage }))
        .catch((e: unknown) => setLoad({ status: 'error', error: e instanceof Error ? e.message : String(e) }));
    });
    return () => task.cancel();
  }, []);
  useEffect(read, [read]);

  const clear = () => {
    haptic.selection();
    Alert.alert(m.media_cache_clear_title(), m.media_cache_clear_body(), [
      { text: m.comum_cancelar(), style: 'cancel' },
      {
        text: m.media_cache_clear(),
        style: 'destructive',
        onPress: () => {
          setClearing(true);
          clearMediaCache()
            // O toast já vibra.
            .then((videoCleared) => {
              if (videoCleared) toast.ok(m.media_cache_cleared());
              else toast.erro(m.media_cache_clear_video_failed());
            })
            .catch((e: unknown) => toast.erro(m.media_cache_clear_failed({ reason: e instanceof Error ? e.message : String(e) })))
            .finally(() => { setClearing(false); setLoad({ status: 'loading' }); read(); });
        },
      },
    ]);
  };

  let body;
  if (load.status === 'loading') body = <Muted>{m.comum_carregando()}</Muted>;
  else if (load.status === 'error') body = (
    <View style={styles.pad}>
      <Text accessibilityRole="alert" style={[styles.help, { color: c.muted }]}>{m.media_cache_read_failed({ reason: load.error })}</Text>
      <View style={styles.row}><Pill icon="RefreshCw" label={m.native_server_retry()} onPress={() => { setLoad({ status: 'loading' }); read(); }} /></View>
    </View>
  );
  else {
    const u = load.usage;
    const empty = u.total === 0 && !u.video;
    body = (
      <View style={styles.pad}>
        <View style={styles.headLine}>
          <Text style={[styles.total, { color: c.text }]}>{fmtBytes(u.total)}</Text>
          <Text style={[styles.help, { color: c.muted }]}>{m.media_cache_of_limit({ limit: fmtBytes(MEDIA_CACHE_LIMIT) })}</Text>
        </View>
        {/* Medidor do teto: cada tipo é um trecho da barra, na ordem da legenda. */}
        <View
          style={[styles.bar, { backgroundColor: c.inset, borderColor: c.border }]}
          accessible
          accessibilityRole="progressbar"
          accessibilityLabel={m.media_cache_title()}
          accessibilityValue={{ min: 0, max: 100, now: Math.min(100, Math.round((u.total * 100) / MEDIA_CACHE_LIMIT)) }}
        >
          {KINDS.map(({ kind, alpha }) => u.byKind[kind] > 0 ? (
            <View key={kind} style={{ width: `${Math.min(100, (u.byKind[kind] * 100) / MEDIA_CACHE_LIMIT)}%`, minWidth: 2, backgroundColor: withAlpha(c.accent, alpha) }} />
          ) : null)}
        </View>
        {empty ? <Text style={[styles.help, { color: c.muted }]}>{m.media_cache_empty()}</Text> : null}
        <View style={styles.legend}>
          {KINDS.map(({ kind, label, alpha }) => (
            <View key={kind} style={styles.legendRow}>
              <View style={[styles.dot, { backgroundColor: withAlpha(c.accent, alpha) }]} />
              <Text style={[styles.legendLabel, { color: c.text }]}>{label()}</Text>
              <Text style={[styles.legendValue, { color: c.muted }]}>{fmtBytes(u.byKind[kind])}</Text>
            </View>
          ))}
          <View style={[styles.legendRow, styles.split, { borderTopColor: c.border }]}>
            <View style={[styles.dot, { borderColor: c.borderStrong, borderWidth: 1 }]} />
            <View style={styles.flex}>
              <Text style={[styles.legendLabel, { color: c.text }]}>{m.media_cache_playback()}</Text>
              <Text style={[styles.help, { color: c.muted }]}>{m.media_cache_playback_help({ limit: fmtBytes(VIDEO_CACHE_LIMIT) })}</Text>
            </View>
            <Text style={[styles.legendValue, { color: c.muted }]}>{u.video == null ? '—' : fmtBytes(u.video)}</Text>
          </View>
        </View>
        <View style={styles.row}>
          <Pill icon="Trash2" label={clearing ? m.media_cache_clearing() : m.media_cache_clear()} onPress={clear} disabled={empty || clearing} />
        </View>
      </View>
    );
  }

  return (
    <>
      <BlockHead title={m.media_cache_title()} help={m.media_cache_help({ limit: fmtBytes(MEDIA_CACHE_LIMIT) })} />
      <Box>{body}</Box>
    </>
  );
}

const styles = StyleSheet.create((theme) => ({
  pad: { padding: theme.base.space[4], gap: theme.base.space[3] },
  row: { flexDirection: 'row' },
  flex: { flex: 1, minWidth: 0 },
  headLine: { flexDirection: 'row', alignItems: 'baseline', gap: theme.base.space[2] },
  total: { fontSize: theme.base.text.lg, fontWeight: theme.base.weight.semibold },
  help: { fontSize: theme.base.text.xs, lineHeight: 18 },
  bar: { height: 10, borderRadius: theme.base.radius.full, borderWidth: 1, flexDirection: 'row', overflow: 'hidden' },
  legend: { gap: theme.base.space[2] },
  legendRow: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2], minHeight: 24 },
  split: { borderTopWidth: 1, paddingTop: theme.base.space[2] },
  dot: { width: 10, height: 10, borderRadius: theme.base.radius.full },
  legendLabel: { flex: 1, fontSize: theme.base.text.sm },
  legendValue: { fontSize: theme.base.text.sm, fontVariant: ['tabular-nums'] },
}));
