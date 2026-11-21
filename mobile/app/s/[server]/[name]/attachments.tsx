import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { ActivityIndicator, FlatList, Pressable, Text, View, useWindowDimensions } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { useLocalSearchParams, useRouter } from 'expo-router';
import * as m from '../../../../src/paraglide/messages';
import {
  conversationAttachments, fileKind, filterByOrigin, groupByDay, isGalleryKind, listUploads, newestFirst,
  type ConversationAttachment, type OriginFilter, type UploadFile,
} from '@hangar/core';
import { useServers } from '../../../../src/stores/servers';
import { chatStore } from '../../../../src/stores/chat';
import { chatJumpRoute, requestChatJump } from '../../../../src/stores/chatJump';
import { superficie } from '../../../../src/theme/superficie';
import { DocumentViewer, type DocumentItem } from '../../../../src/features/attachments/DocumentViewer';
import { GalleryViewer, mediaDayTitle, type GalleryEntry } from '../../../../src/features/attachments/gallery';
import { AudioRow, FileRow, MediaCell, type MediaFileItem } from '../../../../src/features/attachments/MediaFiles';
import { attachmentExpiry, attachmentHeaders, attachmentMtime, attachmentName, attachmentSize, attachmentThumbUri, attachmentUri, type AttachmentMeta } from '../../../../src/features/attachments/attachmentUrls';
import { citedMeta } from '../../../../src/features/attachments/useAttachmentMeta';
import { DictationError, dictateUpload } from '../../../../src/features/ditado/dictationRun';
import { dictationStyleStore } from '../../../../src/features/ditado/ditadoEstiloStore';
import { useSessions } from '../../../../src/stores/sessions';
import { toast } from '../../../../src/ui/Toast';
import { haptic } from '../../../../src/ui/haptics';
import { useRouteBase } from '../../../../src/net/routeBase';

type Tab = 'media' | 'files' | 'audio';
const NO_EVENTS: never[] = [];
const COLS = 3;

const tabOf = (a: Pick<ConversationAttachment, 'kind'>): Tab => (isGalleryKind(a.kind) ? 'media' : a.kind === 'audio' ? 'audio' : 'files');

// Uma linha por anexo: o mesmo arquivo citado em várias respostas aparece uma vez, na mais recente
// (é para ela que "Na conversa" leva). Upload que o histórico carregado não mostra entra pelo `mtime`.
function collect(events: Parameters<typeof conversationAttachments>[0], uploads: UploadFile[], meta: AttachmentMeta): MediaFileItem[] {
  const byRef = new Map<string, ConversationAttachment>();
  for (const a of conversationAttachments(events)) {
    const id = `${a.source}:${a.source === 'transcript' ? a.key : a.ref}`;
    byRef.delete(id);
    byRef.set(id, a);
  }
  for (const f of uploads) {
    const id = `upload:${f.filename}`;
    if (byRef.has(id)) continue;
    byRef.set(id, {
      key: id, eventId: '', ts: f.mtime > 0 ? f.mtime : null, origin: 'user', source: 'upload',
      kind: fileKind(f.filename) ?? 'document', name: f.filename, ref: f.filename,
    });
  }
  const items = [...byRef.values()].map((a): MediaFileItem => ({ ...a, size: attachmentSize(a, meta), mtime: attachmentMtime(a, meta), expiresInDays: attachmentExpiry(a, meta) }));
  return newestFirst(items, (x) => x.ts);
}

// Linha que não mudou mantém o mesmo objeto: mensagem nova não redesenha as células e linhas memo.
function reuse(prev: Map<string, MediaFileItem>, items: MediaFileItem[]): MediaFileItem[] {
  return items.map((x) => {
    const old = prev.get(x.key);
    return old && old.eventId === x.eventId && old.ts === x.ts && old.ref === x.ref && old.size === x.size && old.mtime === x.mtime
      && old.expiresInDays === x.expiresInDays && old.name === x.name ? old : x;
  });
}

type GridRow ={ type: 'day'; key: string; day: number | null } | { type: 'cells'; key: string; items: MediaFileItem[] };

export default function AttachmentsSheet() {
  const { theme } = useUnistyles();
  const router = useRouter();
  const { width } = useWindowDimensions();
  const { server, name } = useLocalSearchParams<{ server: string; name: string }>();
  const sessionName = String(name ?? '');
  const serverId = String(server ?? '');

  const [files, setFiles] = useState<UploadFile[]>([]);
  const [cited, setCited] = useState<AttachmentMeta['cited']>(new Map());
  const [loading, setLoading] = useState(true);
  const [erro, setErro] = useState<string | null>(null);
  const [tab, setTab] = useState<Tab>('media');
  const [filter, setFilter] = useState<OriginFilter>('all');
  const [openKey, setOpenKey] = useState<string | null>(null);
  const [doc, setDoc] = useState<DocumentItem | null>(null);
  const [boxW, setBoxW] = useState<number | null>(null);
  const ready = useServers((s) => s.ready);
  const target = useServers((s) => s.servers.find((x) => x.id === serverId) ?? null);
  // As URLs abaixo levam o endereço da rota: trocar de rede redesenha com o novo.
  const routeBase = useRouteBase(target);
  // A conversa que está embaixo desta folha: o que o agente citou vem do histórico que ela já carregou.
  const events = chatStore(serverId, sessionName).use((s) => (target ? s.events : NO_EVENTS));

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

  const citedPaths = useMemo(() => [...new Set(conversationAttachments(events).filter((a) => a.source === 'cited').map((a) => a.ref))], [events]);
  const citedKey = citedPaths.join('\n');
  useEffect(() => {
    if (!ready || !target || !citedPaths.length) return;
    let alive = true;
    // Tamanho do citado é detalhe: servidor que ainda não mede (ou falha) deixa a linha sem ele.
    citedMeta(sessionName, citedPaths, target).then((c) => { if (alive) setCited(c); }, () => {});
    return () => { alive = false; };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [ready, target, sessionName, citedKey]);

  const meta = useMemo<AttachmentMeta>(() => ({ uploads: new Map(files.map((f) => [f.filename, f])), cited }), [files, cited]);
  const previous = useRef(new Map<string, MediaFileItem>());
  const all = useMemo(() => reuse(previous.current, collect(events, files, meta)), [events, files, meta]);
  useEffect(() => { previous.current = new Map(all.map((x) => [x.key, x])); }, [all]);
  const shown = useMemo(() => filterByOrigin(all, filter), [all, filter]);
  const counts = useMemo(() => {
    const c = { media: 0, files: 0, audio: 0 };
    for (const x of shown) c[tabOf(x)]++;
    return c;
  }, [shown]);
  const list = useMemo(() => shown.filter((x) => tabOf(x) === tab), [shown, tab]);

  const cellSize = Math.floor(((boxW ?? width) - theme.base.space[0.5] * (COLS - 1)) / COLS);
  const gridRows = useMemo<GridRow[]>(() => {
    if (tab !== 'media') return [];
    return groupByDay(list, (x) => x.ts).flatMap((g): GridRow[] => {
      const rows: GridRow[] = [{ type: 'day', key: `day:${g.day ?? 'none'}`, day: g.day }];
      for (let i = 0; i < g.items.length; i += COLS) {
        const items = g.items.slice(i, i + COLS);
        rows.push({ type: 'cells', key: `cells:${items[0].key}`, items });
      }
      return rows;
    });
  }, [list, tab]);

  // A galeria passa pelas mídias na ordem da grade, com o mesmo filtro.
  const entries = useMemo<GalleryEntry[]>(() => (target && tab === 'media' ? list.map((a) => ({
    key: a.key,
    uri: attachmentUri(a, sessionName, target),
    thumbUri: attachmentThumbUri(a, sessionName, target),
    headers: attachmentHeaders(a, target),
    name: attachmentName(a),
    kind: a.kind === 'video' ? 'video' as const : 'image' as const,
    origin: a.origin,
    ts: a.ts,
    size: a.size,
    expiresInDays: a.expiresInDays,
    eventId: a.eventId || undefined,
  })) : []),
  // eslint-disable-next-line react-hooks/exhaustive-deps
  [list, tab, target, sessionName, routeBase]);

  const guard = useCallback(() => {
    if (!target) { toast.erro(m.chat_servidor_removido()); return false; }
    // `fileAuthHeader` sempre monta o cabeçalho, mesmo com o token vazio.
    if (!target.token) { toast.erro(m.sessao_expirada()); return false; }
    return true;
  }, [target]);

  const openMedia = useCallback((item: MediaFileItem) => {
    if (!guard()) return;
    haptic.selection();
    setOpenKey(item.key);
  }, [guard]);

  const openDoc = useCallback((item: MediaFileItem) => {
    if (!guard() || !target || (item.kind !== 'pdf' && item.kind !== 'html')) return;
    setDoc({ uri: attachmentUri(item, sessionName, target), headers: attachmentHeaders(item, target), name: attachmentName(item), kind: item.kind });
  }, [guard, target, sessionName]);

  const showInChat = useCallback((entry: GalleryEntry) => {
    if (!entry.eventId) return;
    setOpenKey(null);
    requestChatJump(chatJumpRoute(serverId, sessionName), entry.eventId);
    router.back();
  }, [serverId, sessionName, router]);

  // Volta para a conversa: o Composer mostra "transcrevendo" e recebe o texto ou o motivo da falha.
  const retranscribe = useCallback((f: MediaFileItem) => {
    const target = useServers.getState().servers.find((s) => s.id === serverId);
    if (!target) { toast.erro(m.chat_servidor_removido()); return; }
    const sessions = useSessions.getState();
    const transcript = (sessions.byServerRecord?.[serverId]?.find((x) => x.name === sessionName)?.jsonl
      ?? sessions.rows.find((x) => x.serverId === serverId && x.name === sessionName)?.jsonl) || null;
    const style = dictationStyleStore(target).getState();
    let run: Promise<unknown>;
    try {
      run = dictateUpload(target, serverId, sessionName, transcript, f.ref, style.pronto ? style.valor : undefined);
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
      <View style={[styles.container, styles.centerFill, { backgroundColor: theme.tokens.bg.base }]}>
        <ActivityIndicator color={theme.tokens.text.muted} />
        <Text style={[styles.muted, { color: theme.tokens.text.muted }]}>{m.comum_carregando()}</Text>
      </View>
    );
  }

  if (!target) {
    return (
      <View style={[styles.container, styles.centerFill, { backgroundColor: theme.tokens.bg.base }]}>
        <Text style={[styles.err, { color: theme.tokens.status.error }]} accessibilityRole="alert">
          {m.chat_servidor_removido()}
        </Text>
      </View>
    );
  }

  const tabs: { id: Tab; label: string }[] = [
    { id: 'media', label: m.media_tab_media({ n: counts.media }) },
    { id: 'files', label: m.media_tab_files({ n: counts.files }) },
    { id: 'audio', label: m.media_tab_audio({ n: counts.audio }) },
  ];
  const chips: { id: OriginFilter; label: string }[] = [
    { id: 'all', label: m.media_filter_all() },
    { id: 'agent', label: m.media_filter_agent() },
    { id: 'user', label: m.media_filter_user() },
  ];

  const body = loading ? (
    <View style={styles.center}>
      <ActivityIndicator color={theme.tokens.text.secondary} />
      <Text style={[styles.muted, { color: theme.tokens.text.muted }]}>{m.comum_carregando()}</Text>
    </View>
  ) : erro ? (
    <View style={styles.center}>
      <Text style={[styles.err, { color: theme.tokens.status.error }]} accessibilityRole="alert">{m.anexos_erro_listar()} {erro}</Text>
      <Pressable onPress={() => void load()} style={[styles.retryBtn, { borderColor: theme.tokens.border.subtle }]} accessibilityRole="button">
        <Text style={[styles.retryText, { color: theme.tokens.accent.base }]}>{m.lista_tentar_novamente()}</Text>
      </Pressable>
    </View>
  ) : all.length === 0 ? (
    <Text style={[styles.muted, { color: theme.tokens.text.muted }]}>{m.anexos_nenhum()}</Text>
  ) : list.length === 0 ? (
    <Text style={[styles.muted, { color: theme.tokens.text.muted }]}>{m.media_empty()}</Text>
  ) : tab === 'media' ? (
    <FlatList
      data={gridRows}
      keyExtractor={(r) => r.key}
      contentContainerStyle={styles.listPad}
      renderItem={({ item: r }) => (r.type === 'day' ? (
        <Text style={[styles.day, { color: theme.tokens.text.muted }]} accessibilityRole="header">{mediaDayTitle(r.day)}</Text>
      ) : (
        <View style={styles.gridRow}>
          {r.items.map((it) => <MediaCell key={it.key} item={it} size={cellSize} server={target} sessionName={sessionName} onOpen={openMedia} />)}
        </View>
      ))}
    />
  ) : (
    <FlatList
      data={list}
      keyExtractor={(x) => x.key}
      contentContainerStyle={styles.listPad}
      renderItem={({ item }) => (tab === 'files'
        ? <FileRow item={item} server={target} sessionName={sessionName} serverId={serverId} onOpenDoc={openDoc} />
        : <AudioRow item={item} server={target} sessionName={sessionName} serverId={serverId}
            onRetranscribe={item.source === 'upload' ? retranscribe : undefined} />)}
    />
  );

  return (
    <View style={[styles.container, { backgroundColor: theme.tokens.bg.base }]} onLayout={(e) => setBoxW(e.nativeEvent.layout.width - theme.base.space[4] * 2)}>
      <Text style={[styles.title, { color: theme.tokens.text.primary }]} accessibilityRole="header">
        {m.media_title()}
        {all.length ? <Text style={[styles.count, { color: theme.tokens.text.secondary, backgroundColor: superficie(theme, 0.8) }]}> {all.length}</Text> : null}
      </Text>
      <View style={[styles.seg, { backgroundColor: superficie(theme, 0.8) }]} accessibilityRole="tablist">
        {tabs.map((t) => (
          <Pressable key={t.id} onPress={() => { haptic.selection(); setOpenKey(null); setTab(t.id); }}
            style={[styles.segItem, tab === t.id && { backgroundColor: theme.tokens.bg.elevated }]}
            accessibilityRole="tab" accessibilityState={{ selected: tab === t.id }}>
            <Text style={[styles.segTxt, { color: tab === t.id ? theme.tokens.text.primary : theme.tokens.text.secondary }]} numberOfLines={1}>{t.label}</Text>
          </Pressable>
        ))}
      </View>
      <View style={styles.chips}>
        {chips.map((c) => (
          <Pressable key={c.id} onPress={() => { haptic.selection(); setOpenKey(null); setFilter(c.id); }} hitSlop={6}
            style={[styles.chip, { borderColor: filter === c.id ? theme.tokens.accent.base : theme.tokens.border.subtle },
              filter === c.id && { backgroundColor: theme.tokens.accent.dim }]}
            accessibilityRole="button" accessibilityState={{ selected: filter === c.id }}>
            <Text style={[styles.chipTxt, { color: filter === c.id ? theme.tokens.text.primary : theme.tokens.text.secondary }]}>{c.label}</Text>
          </Pressable>
        ))}
      </View>
      <View style={styles.body}>{body}</View>

      {openKey !== null ? (
        <GalleryViewer entries={entries} openKey={openKey} onOpenKey={setOpenKey} onClose={() => setOpenKey(null)}
          onShowInChat={showInChat} modal={false} />
      ) : null}
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
  centerFill: {
    alignItems: 'center',
    justifyContent: 'center',
    gap: theme.base.space[2],
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
  seg: {
    flexDirection: 'row',
    padding: theme.base.space[0.5],
    borderRadius: theme.base.radius.sm,
    gap: theme.base.space[0.5],
  },
  segItem: {
    flex: 1,
    minHeight: 44,
    alignItems: 'center',
    justifyContent: 'center',
    borderRadius: theme.base.radius.sm,
  },
  segTxt: {
    fontSize: theme.base.text.xs,
    fontWeight: theme.base.weight.medium,
    fontVariant: ['tabular-nums'],
  },
  chips: {
    flexDirection: 'row',
    gap: theme.base.space[2],
  },
  chip: {
    minHeight: 32,
    justifyContent: 'center',
    paddingHorizontal: theme.base.space[3],
    borderRadius: theme.base.radius.full,
    borderWidth: 1,
  },
  chipTxt: {
    fontSize: theme.base.text.xs,
  },
  body: {
    flex: 1,
  },
  listPad: {
    paddingBottom: theme.base.space[4],
  },
  day: {
    fontSize: theme.base.text.xxs,
    marginTop: theme.base.space[3],
    marginBottom: theme.base.space[1.5],
  },
  gridRow: {
    flexDirection: 'row',
    gap: theme.base.space[0.5],
    marginBottom: theme.base.space[0.5],
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
}));
