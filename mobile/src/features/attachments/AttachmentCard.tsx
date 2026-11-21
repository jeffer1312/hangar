import { ActivityIndicator, Pressable, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { Image } from 'expo-image';
import { fileKind, fmtBytes, relativeTime } from '@hangar/core';
import type { UploadFile } from '@hangar/core';
import * as m from '../../paraglide/messages';
import { uploadUrlNative, fileAuthHeader } from '@hangar/core';
import { Icon, type IconName } from '../../ui/Icon';
import { superficie } from '../../theme/superficie';
import { useServers } from '../../stores/servers';
import { useRouteBase } from '../../net/routeBase';
import { thumbWidthFor } from '../../chat/thumbs';

interface Props {
  file: UploadFile;
  sessionName: string;
  serverId: string;
  onPress: () => void;
  /** O documento está sendo baixado para entregar ao sistema. */
  busy?: boolean;
}

// Maior largura do cartão na grade (`maxWidth` abaixo): a miniatura pedida cobre o cartão inteiro.
const TILE_MAX = 140;

// prazo em texto curto — porte de AttachmentsSheet.svelte:52-56
function prazo(d: number | null): { txt: string; urgente: boolean } {
  if (d === null) return { txt: m.anexos_sem_expiracao(), urgente: false };
  if (d <= 0) return { txt: m.anexos_vencido(), urgente: true };
  if (d < 1) return { txt: m.anexos_expira_h({ n: Math.max(1, Math.round(d * 24)) }), urgente: true };
  return { txt: m.anexos_expira_d({ n: Math.round(d) }), urgente: d <= 3 };
}

function icone(f: UploadFile): IconName {
  const k = fileKind(f.filename);
  return k === 'pdf' ? 'FileText' : k === 'html' ? 'Globe' : k === 'audio' ? 'Music' : 'Paperclip';
}

export function AttachmentCard({ file, sessionName, serverId, onPress, busy = false }: Props) {
  const { theme } = useUnistyles();
  const server = useServers((s) => s.servers.find((x) => x.id === serverId) ?? null);
  useRouteBase(server);
  if (!server) return <Text style={{ color: theme.tokens.status.error }} accessibilityRole="alert">{m.chat_servidor_removido()}</Text>;
  const kind = fileKind(file.filename);
  const p = prazo(file.expires_in_days);
  const uri = uploadUrlNative(sessionName, file.filename, server, thumbWidthFor(file.filename, TILE_MAX));
  const headers = fileAuthHeader(server);
  return (
    <View style={styles.item}>
      {kind === 'image' ? (
        <Pressable onPress={onPress} style={styles.tile} accessibilityLabel={m.anexos_ver({ n: file.filename })} accessibilityRole="button">
          <Image source={{ uri, headers }} style={styles.media} contentFit="cover" transition={200} />
        </Pressable>
      ) : kind === 'video' ? (
        <Pressable onPress={onPress} style={styles.tile} accessibilityLabel={m.anexos_tocar({ nome: file.filename })} accessibilityRole="button">
          <View style={[styles.tileChip, { backgroundColor: superficie(theme, 0.8) }]}>
            <Icon name="Play" size={28} color={theme.tokens.text.secondary} />
          </View>
        </Pressable>
      ) : (
        <Pressable onPress={onPress} disabled={busy} style={styles.tile} accessibilityLabel={m.anexos_ver({ n: file.filename })}
          accessibilityRole="button" accessibilityState={{ busy }}>
          <View style={[styles.tileChip, { backgroundColor: superficie(theme, 0.8) }]}>
            {busy
              ? <ActivityIndicator color={theme.tokens.text.secondary} />
              : <Icon name={icone(file)} size={28} color={theme.tokens.text.secondary} />}
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
    gap: theme.base.space[0.5],
  },
  tile: {
    aspectRatio: 1,
    width: '100%',
    borderRadius: theme.base.radius.md,
    overflow: 'hidden',
    backgroundColor: superficie(theme, 0.8),
  },
  tileChip: {
    flex: 1,
    alignItems: 'center',
    justifyContent: 'center',
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
    fontSize: theme.base.text.xxs,
    fontVariant: ['tabular-nums'],
  },
  prazo: {
    fontSize: theme.base.text.xxs,
  },
}));
