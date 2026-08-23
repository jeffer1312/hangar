import { Pressable, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { Image } from 'expo-image';
import { fileKind, fmtBytes, relativeTime } from '@hangar/core';
import type { UploadFile } from '@hangar/core';
import * as m from '../../paraglide/messages';
import { uploadUrl } from '@hangar/core';

interface Props {
  file: UploadFile;
  sessionName: string;
  onPress: () => void;
}

// prazo em texto curto — porte de AttachmentsSheet.svelte:52-56
function prazo(d: number | null): { txt: string; urgente: boolean } {
  if (d === null) return { txt: m.anexos_sem_expiracao(), urgente: false };
  if (d <= 0) return { txt: m.anexos_vencido(), urgente: true };
  if (d < 1) return { txt: m.anexos_expira_h({ n: Math.max(1, Math.round(d * 24)) }), urgente: true };
  return { txt: m.anexos_expira_d({ n: Math.round(d) }), urgente: d <= 3 };
}

function icone(f: UploadFile): string {
  const k = fileKind(f.filename);
  return k === 'pdf' ? '📄' : k === 'html' ? '🌐' : k === 'audio' ? '🎵' : '📎';
}

export function AttachmentCard({ file, sessionName, onPress }: Props) {
  const { theme } = useUnistyles();
  const kind = fileKind(file.filename);
  const p = prazo(file.expires_in_days);
  const uri = uploadUrl(sessionName, file.filename);
  return (
    <View style={styles.item}>
      {kind === 'image' ? (
        <Pressable onPress={onPress} style={styles.tile} accessibilityLabel={m.anexos_ver({ n: file.filename })} accessibilityRole="button">
          <Image source={{ uri }} style={styles.media} contentFit="cover" transition={200} />
        </Pressable>
      ) : kind === 'video' ? (
        <Pressable onPress={onPress} style={styles.tile} accessibilityLabel={m.anexos_ver({ n: file.filename })} accessibilityRole="button">
          <View style={[styles.tileChip, { backgroundColor: theme.tokens.bg.elevated }]}>
            <Text style={styles.chipIco}>▶</Text>
          </View>
        </Pressable>
      ) : (
        <Pressable onPress={onPress} style={styles.tile} accessibilityLabel={m.anexos_ver({ n: file.filename })} accessibilityRole="button">
          <View style={[styles.tileChip, { backgroundColor: theme.tokens.bg.elevated }]}>
            <Text style={styles.chipIco}>{icone(file)}</Text>
          </View>
        </Pressable>
      )}
      <Text style={[styles.nome, { color: theme.tokens.text.primary }]} numberOfLines={1}>
        {file.filename}
      </Text>
      <Text style={[styles.meta, { color: theme.tokens.text.muted }]} numberOfLines={1}>
        {fmtBytes(file.size)} · {relativeTime(file.mtime)}
      </Text>
      <Text style={[styles.prazo, { color: p.urgente ? theme.tokens.status.warning : theme.tokens.text.muted }]} numberOfLines={1}>
        {p.txt}
      </Text>
    </View>
  );
}

const styles = StyleSheet.create((theme) => ({
  item: {
    width: '30%',
    minWidth: 96,
    maxWidth: 140,
    flexGrow: 1,
    gap: 2,
  },
  tile: {
    aspectRatio: 1,
    width: '100%',
    borderRadius: theme.base.radius.md,
    overflow: 'hidden',
    backgroundColor: theme.tokens.bg.elevated,
  },
  tileChip: {
    flex: 1,
    alignItems: 'center',
    justifyContent: 'center',
  },
  chipIco: {
    fontSize: 28,
  },
  media: {
    width: '100%',
    height: '100%',
  },
  nome: {
    fontSize: theme.base.text.xs,
    overflow: 'hidden',
  },
  meta: {
    fontSize: 11,
    fontVariant: ['tabular-nums'],
  },
  prazo: {
    fontSize: 11,
  },
}));
