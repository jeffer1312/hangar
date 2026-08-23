import { useCallback, useEffect, useState } from 'react';
import { ActivityIndicator, Pressable, ScrollView, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { useLocalSearchParams, useRouter } from 'expo-router';
import { WebView } from 'react-native-webview';
import * as m from '../../../../src/paraglide/messages';
import { fileUrl, listUploads, uploadUrl } from '@hangar/core';
import type { UploadFile } from '@hangar/core';
import { AttachmentCard } from '../../../../src/features/attachments/AttachmentCard';
import { Lightbox } from '../../../../src/features/attachments/Lightbox';
import { fileKind } from '@hangar/core';

export default function AttachmentsSheet() {
  const { theme } = useUnistyles();
  const router = useRouter();
  const { server, name } = useLocalSearchParams<{ server: string; name: string }>();
  const sessionName = String(name ?? '');
  const serverId = String(server ?? '');

  const [files, setFiles] = useState<UploadFile[]>([]);
  const [loading, setLoading] = useState(true);
  const [erro, setErro] = useState<string | null>(null);
  const [lightbox, setLightbox] = useState<UploadFile | null>(null);
  const [docUrl, setDocUrl] = useState<{ url: string; title: string } | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    setErro(null);
    try {
      const r = await listUploads(sessionName);
      setFiles(r.files);
    } catch (e) {
      setErro(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  }, [sessionName]);

  useEffect(() => {
    void load();
  }, [load]);

  const handlePress = useCallback(
    (f: UploadFile) => {
      const k = fileKind(f.filename);
      if (k === 'image') {
        setLightbox(f);
        return;
      }
      if (k === 'html' || k === 'pdf') {
        // Task 12 route exists as stub: navigate there; until then fallback to inline WebView
        // We try push, but also keep inline fallback for visual parity if route empty.
        const url = fileUrl(sessionName, f.filename);
        // If files route is still stub, WebView inline gives visual proof; otherwise push navigates.
        // Prefer push to keep navigation consistent; fallback is docUrl inline.
        try {
          router.push(`/s/${serverId}/${sessionName}/files?path=${encodeURIComponent(f.filename)}` as never);
          return;
        } catch {
          setDocUrl({ url, title: f.filename });
          return;
        }
      }
      // video/audio/other: open via system viewer (fileUrl in browser) – for parity show inline WebView for video
      if (k === 'video' || k === 'audio') {
        const url = fileUrl(sessionName, f.filename);
        setDocUrl({ url, title: f.filename });
        return;
      }
      const url = uploadUrl(sessionName, f.filename);
      setDocUrl({ url, title: f.filename });
    },
    [sessionName, serverId, router],
  );

  if (docUrl) {
    return (
      <View style={[styles.container, { backgroundColor: theme.tokens.bg.base }]}>
        <View style={[styles.bar, { borderBottomColor: theme.tokens.border.subtle }]}>
          <Text style={[styles.docTitle, { color: theme.tokens.text.primary }]} numberOfLines={1}>
            {docUrl.title}
          </Text>
          <Pressable onPress={() => setDocUrl(null)} style={[styles.docBtn, { borderColor: theme.tokens.border.subtle }]} accessibilityRole="button" accessibilityLabel={m.anexos_fechar_visualizacao()}>
            <Text style={{ color: theme.tokens.text.primary }}>✕</Text>
          </Pressable>
        </View>
        <WebView source={{ uri: docUrl.url }} style={styles.webview} />
      </View>
    );
  }

  return (
    <View style={[styles.container, { backgroundColor: theme.tokens.bg.base }]}>
      <Text style={[styles.title, { color: theme.tokens.text.primary }]}>
        {m.ctx_anexos()}
        {files.length ? <Text style={[styles.count, { color: theme.tokens.text.secondary, backgroundColor: theme.tokens.bg.elevated }]}> {files.length}</Text> : null}
      </Text>

      {loading ? (
        <View style={styles.center}>
          <ActivityIndicator color={theme.tokens.text.secondary} />
          <Text style={[styles.muted, { color: theme.tokens.text.muted }]}>{m.comum_carregando()}</Text>
        </View>
      ) : erro ? (
        <View style={styles.center}>
          <Text style={[styles.err, { color: theme.tokens.status.error }]}>{m.anexos_erro_listar()} {erro}</Text>
          <Pressable onPress={() => void load()} style={[styles.retryBtn, { borderColor: theme.tokens.border.subtle }]} accessibilityRole="button">
            <Text style={[styles.retryText, { color: theme.tokens.accent.base }]}>{m.lista_tentar_novamente()}</Text>
          </Pressable>
        </View>
      ) : files.length === 0 ? (
        <Text style={[styles.muted, { color: theme.tokens.text.muted }]}>{m.anexos_nenhum()}</Text>
      ) : (
        <ScrollView contentContainerStyle={styles.grid}>
          {files.map((f) => (
            <AttachmentCard key={f.filename} file={f} sessionName={sessionName} onPress={() => handlePress(f)} />
          ))}
        </ScrollView>
      )}

      <Lightbox
        visible={!!lightbox}
        uri={lightbox ? uploadUrl(sessionName, lightbox.filename) : ''}
        filename={lightbox?.filename ?? ''}
        onClose={() => setLightbox(null)}
      />
    </View>
  );
}

const styles = StyleSheet.create((theme) => ({
  container: {
    flex: 1,
    padding: theme.base.space[4],
    gap: theme.base.space[3],
  },
  title: {
    fontSize: theme.base.text.base,
    fontWeight: '600',
  },
  count: {
    fontSize: 11,
    fontWeight: '700',
    paddingHorizontal: 8,
    paddingVertical: 2,
    borderRadius: theme.base.radius.full,
    overflow: 'hidden',
  },
  center: {
    flex: 1,
    alignItems: 'center',
    justifyContent: 'center',
    gap: theme.base.space[2],
    paddingVertical: theme.base.space[4],
  },
  muted: {
    fontSize: theme.base.text.sm,
    textAlign: 'center',
  },
  err: {
    fontSize: theme.base.text.sm,
    textAlign: 'center',
  },
  retryBtn: {
    borderWidth: 1,
    borderRadius: theme.base.radius.full,
    paddingHorizontal: theme.base.space[3],
    paddingVertical: 6,
  },
  retryText: {
    fontSize: theme.base.text.sm,
    fontWeight: '600',
  },
  grid: {
    flexDirection: 'row',
    flexWrap: 'wrap',
    gap: theme.base.space[3],
    paddingBottom: theme.base.space[4],
  },
  bar: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: theme.base.space[2],
    paddingBottom: theme.base.space[2],
    borderBottomWidth: 1,
  },
  docTitle: {
    flex: 1,
    fontSize: theme.base.text.sm,
    fontWeight: '600',
  },
  docBtn: {
    width: 36,
    height: 36,
    borderRadius: 18,
    borderWidth: 1,
    alignItems: 'center',
    justifyContent: 'center',
  },
  webview: {
    flex: 1,
  },
}));
