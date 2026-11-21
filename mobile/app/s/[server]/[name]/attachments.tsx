import { useCallback, useEffect, useState } from 'react';
import { ActivityIndicator, Pressable, ScrollView, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { useLocalSearchParams, useRouter } from 'expo-router';
import * as m from '../../../../src/paraglide/messages';
import { fileAuthHeader, listUploads, uploadUrlNative } from '@hangar/core';
import type { UploadFile } from '@hangar/core';
import { AttachmentCard } from '../../../../src/features/attachments/AttachmentCard';
import { Lightbox } from '../../../../src/features/attachments/Lightbox';
import { fileKind } from '@hangar/core';
import { useServers } from '../../../../src/stores/servers';
import { superficie } from '../../../../src/theme/superficie';
import { AudioChip } from '../../../../src/chat/AudioChip';
import { DocumentViewer, type DocumentItem } from '../../../../src/features/attachments/DocumentViewer';
import { DictationError, dictateUpload } from '../../../../src/features/ditado/dictationRun';
import { dictationStyleStore } from '../../../../src/features/ditado/ditadoEstiloStore';
import { useSessions } from '../../../../src/stores/sessions';
import { toast } from '../../../../src/ui/Toast';
import { haptic } from '../../../../src/ui/haptics';
import { useRouteBase } from '../../../../src/net/routeBase';

const isMedia = (name: string) => {
  const kind = fileKind(name);
  return kind === 'image' || kind === 'video';
};

export default function AttachmentsSheet() {
  const { theme } = useUnistyles();
  const router = useRouter();
  const { server, name } = useLocalSearchParams<{ server: string; name: string }>();
  const sessionName = String(name ?? '');
  const serverId = String(server ?? '');

  const [files, setFiles] = useState<UploadFile[]>([]);
  const [loading, setLoading] = useState(true);
  const [erro, setErro] = useState<string | null>(null);
  const [lightbox, setLightbox] = useState<string | null>(null);
  const [doc, setDoc] = useState<DocumentItem | null>(null);
  const [delivering, setDelivering] = useState<string | null>(null);
  const ready = useServers((s) => s.ready);
  const target = useServers((s) => s.servers.find((x) => x.id === serverId) ?? null);
  // As URLs abaixo levam o endereço da rota: trocar de rede redesenha com o novo.
  useRouteBase(target);

  const load = useCallback(async () => {
    if (!target) { setErro(m.chat_servidor_removido()); setLoading(false); return; }
    setLoading(true);
    setErro(null);
    try {
      const r = await listUploads(sessionName, target);
      setFiles(r.files);
    } catch (e) {
      setErro(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  }, [sessionName, target]);

  useEffect(() => {
    if (!ready) return;
    void load();
  }, [load, ready]);

  const audios = files.filter((f) => fileKind(f.filename) === 'audio');
  const others = files.filter((f) => fileKind(f.filename) !== 'audio');
  // Fotos e vídeos formam uma galeria só no visor, como na conversa.
  const media = others.filter((f) => isMedia(f.filename));

  // Documento (docx, planilha, zip, csv…): o WebView não lê; vai como no card da conversa, pela folha
  // do sistema no iOS ou pelo app que abre o tipo no Android.
  const deliver = useCallback(async (f: UploadFile, uri: string, headers: Record<string, string>) => {
    haptic.selection();
    setDelivering(f.filename);
    try {
      const { deliverFile } = await import('../../../../src/features/attachments/mediaCache');
      const r = await deliverFile(uri, headers, f.filename);
      if (r === 'opened') haptic.success();
      else if (r === 'empty' || r === 'no-app') {
        haptic.error();
        toast.erro(`${f.filename}: ${r === 'empty' ? m.file_card_empty() : m.file_card_no_app()}`);
      }
    } catch (e) {
      haptic.error();
      const missing = /status:? 404\b/.test(e instanceof Error ? e.message : String(e));
      toast.erro(`${f.filename}: ${missing ? m.file_card_missing() : m.file_card_failed()}`);
    } finally {
      setDelivering(null);
    }
  }, []);

  const handlePress = useCallback(
    (f: UploadFile) => {
      if (!target) { toast.erro(m.chat_servidor_removido()); return; }
      if (Object.keys(fileAuthHeader(target)).length === 0) { toast.erro(m.sessao_expirada()); return; }
      const k = fileKind(f.filename);
      if (k === 'image' || k === 'video') {
        setLightbox(f.filename);
        return;
      }
      const uri = uploadUrlNative(sessionName, f.filename, target);
      if (k === 'html' || k === 'pdf') {
        setDoc({ uri, headers: fileAuthHeader(target), name: f.filename, kind: k });
        return;
      }
      if (delivering) return;
      void deliver(f, uri, fileAuthHeader(target));
    },
    [sessionName, target, delivering, deliver],
  );

  // Volta para a conversa: o Composer mostra "transcrevendo" e recebe o texto ou o motivo da falha.
  const retranscribe = useCallback((f: UploadFile) => {
    const target = useServers.getState().servers.find((s) => s.id === serverId);
    if (!target) { toast.erro(m.chat_servidor_removido()); return; }
    const sessions = useSessions.getState();
    const transcript = (sessions.byServerRecord?.[serverId]?.find((x) => x.name === sessionName)?.jsonl
      ?? sessions.rows.find((x) => x.serverId === serverId && x.name === sessionName)?.jsonl) || null;
    const style = dictationStyleStore(target).getState();
    let run: Promise<unknown>;
    try {
      run = dictateUpload(target, serverId, sessionName, transcript, f.filename, style.pronto ? style.valor : undefined);
    } catch (e) {
      toast.erro(e instanceof Error ? e.message : m.composer_falha_transcricao());
      return;
    }
    // A falha guardada já fica no aviso do ditado da conversa; só a que não foi guardada vira toast.
    run.catch((e) => {
      if (e instanceof DictationError && (e.lost || e.storageIssue)) {
        toast.erro(e.storageIssue ? `${e.message} (${e.storageIssue})` : e.message);
      }
    });
    router.back();
  }, [serverId, sessionName, router]);

  if (!ready) {
    return (
      <View style={[styles.container, { backgroundColor: theme.tokens.bg.base, flex: 1, alignItems: 'center', justifyContent: 'center' }]}>
        <ActivityIndicator color={theme.tokens.text.muted} />
        <Text style={[styles.muted, { color: theme.tokens.text.muted }]}>{m.comum_carregando()}</Text>
      </View>
    );
  }

  if (!target) {
    return (
      <View style={[styles.container, { backgroundColor: theme.tokens.bg.base, flex: 1, alignItems: 'center', justifyContent: 'center', gap: theme.base.space[2] }]}>
        <Text style={[styles.err, { color: theme.tokens.status.error }]} accessibilityRole="alert">
          {m.chat_servidor_removido()}
        </Text>
      </View>
    );
  }

  // O visor guarda o nome, não o índice: a lista pode recarregar com ele aberto.
  const lightboxIndex = lightbox === null ? -1 : media.findIndex((x) => x.filename === lightbox);
  const current = lightboxIndex >= 0 ? media[lightboxIndex] : null;
  const step = (dir: 1 | -1) => {
    if (lightboxIndex < 0) return;
    const next = media[Math.min(media.length - 1, Math.max(0, lightboxIndex + dir))];
    if (next) setLightbox(next.filename);
  };

  return (
    <View style={[styles.container, { backgroundColor: theme.tokens.bg.base }]}>
      <Text style={[styles.title, { color: theme.tokens.text.primary }]}>
        {m.ctx_anexos()}
        {files.length ? <Text style={[styles.count, { color: theme.tokens.text.secondary, backgroundColor: superficie(theme, 0.8) }]}> {files.length}</Text> : null}
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
        <ScrollView contentContainerStyle={styles.list}>
          {audios.map((f) => (
            <View key={f.filename} style={styles.audioRow}>
              <AudioChip uri={uploadUrlNative(sessionName, f.filename, target)} headers={fileAuthHeader(target)} name={f.filename} />
              <Pressable onPress={() => retranscribe(f)} hitSlop={8} style={[styles.retryBtn, { borderColor: theme.tokens.border.subtle }]}
                accessibilityRole="button" accessibilityLabel={`${m.composer_transcrever_de_novo()}: ${f.filename}`}>
                <Text style={[styles.retryText, { color: theme.tokens.accent.base }]}>{m.composer_transcrever_de_novo()}</Text>
              </Pressable>
            </View>
          ))}
          <View style={styles.grid}>
            {others.map((f) => (
              <AttachmentCard key={f.filename} file={f} sessionName={sessionName} serverId={serverId}
                busy={delivering === f.filename} onPress={() => handlePress(f)} />
            ))}
          </View>
        </ScrollView>
      )}

      <Lightbox
        visible={!!current}
        uri={current ? uploadUrlNative(sessionName, current.filename, target) : ''}
        headers={fileAuthHeader(target)}
        filename={current?.filename ?? ''}
        kind={current && fileKind(current.filename) === 'video' ? 'video' : 'image'}
        counter={current && media.length > 1 ? `${lightboxIndex + 1}/${media.length}` : undefined}
        onStep={media.length > 1 ? step : undefined}
        onClose={() => setLightbox(null)}
      />
      <DocumentViewer doc={doc} onClose={() => setDoc(null)} />
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
    fontWeight: theme.base.weight.semibold,
  },
  count: {
    fontSize: theme.base.text.xxs,
    fontWeight: theme.base.weight.bold,
    paddingHorizontal: theme.base.space[2],
    paddingVertical: theme.base.space[0.5],
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
    paddingVertical: theme.base.space[1.5],
  },
  retryText: {
    fontSize: theme.base.text.sm,
    fontWeight: theme.base.weight.semibold,
  },
  list: {
    gap: theme.base.space[3],
    paddingBottom: theme.base.space[4],
  },
  grid: {
    flexDirection: 'row',
    flexWrap: 'wrap',
    gap: theme.base.space[3],
  },
  audioRow: {
    flexDirection: 'row',
    flexWrap: 'wrap',
    alignItems: 'center',
    gap: theme.base.space[2],
  },
}));
