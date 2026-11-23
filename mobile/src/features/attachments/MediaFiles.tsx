import { memo, useCallback, useEffect, useRef, useState } from 'react';
import { Pressable, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import type { UnistylesThemes } from 'react-native-unistyles';
import { Image } from 'expo-image';
import type { File } from 'expo-file-system';
import { attachmentFolder, fmtBytes, registrarDiag, type ConversationAttachment, type Server } from '@hangar/core';
import * as m from '../../paraglide/messages';
import { Icon } from '../../ui/Icon';
import { toast } from '../../ui/Toast';
import { haptic } from '../../ui/haptics';
import { superficie } from '../../theme/superficie';
import { AudioChip } from '../../chat/AudioChip';
import { useRouteBase } from '../../net/routeBase';
import { attachmentHeaders, attachmentName, attachmentThumbUri, attachmentUri, keptKey, type KeptVersion } from './attachmentUrls';
import { deliverLocalFile, downloadKept, isAbort, isMissing, keptCopy, type DownloadProgress } from './mediaCache';
import { DownloadRing } from './DownloadRing';
import { percentOf, progressText, useDownload } from './useDownload';
import { expiryText } from './gallery';
import { originLabel } from './origin';
import { MediaCellFrame } from './MediaFilesParts';

type Theme = UnistylesThemes[keyof UnistylesThemes];

/** Um item da tela: o anexo da conversa (ou o upload fora do histórico carregado) com o que o servidor sabe dele. */
export interface MediaFileItem extends ConversationAttachment {
  size: number | null;
  /** Data do arquivo no servidor; null/ausente quando ele não informa. */
  mtime?: number | null;
  /** Prazo do upload em dias (null = sem prazo); ausente quando não é upload conhecido. */
  expiresInDays?: number | null;
}

// ─── Mídia ─────────────────────────────────────────────────────────────────────────────────────

export const MediaCell = memo(function MediaCell({ item, size, server, sessionName, onOpen }: {
  item: MediaFileItem; size: number; server: Server; sessionName: string; onOpen: (item: MediaFileItem) => void;
}) {
  const name = attachmentName(item);
  // A troca de rota (rede local ↔ Tailscale) redesenha a célula mesmo com o `memo`; a URL sai do `baseOf`.
  useRouteBase(server);
  const thumb = attachmentThumbUri(item, sessionName, server, size) ?? attachmentUri(item, sessionName, server);
  const [failed, setFailed] = useState<string | null>(null);
  const broken = failed === thumb;
  return (
    <MediaCellFrame size={size} origin={item.origin} onPress={() => onOpen(item)}
      placeholder={item.kind === 'image' && !broken ? undefined : broken ? 'ImageOff' : 'Play'}
      label={item.kind === 'video' ? m.anexos_tocar({ nome: name }) : m.anexos_ver({ n: name })}>
      <Image source={{ uri: thumb, headers: attachmentHeaders(item, server) }} style={styles.fill} contentFit="cover" transition={150}
        onError={() => setFailed(thumb)} />
    </MediaCellFrame>
  );
});

// ─── Arquivos ──────────────────────────────────────────────────────────────────────────────────

function extOf(name: string): string {
  const ext = name.includes('.') ? name.split('.').pop()!.toLowerCase() : '';
  return ext;
}

function badgeColor(ext: string, theme: Theme): string {
  if (ext === 'pdf') return theme.tokens.status.error;
  if (/^(xlsx?|ods|csv|tsv)$/.test(ext)) return theme.tokens.status.success;
  if (/^(html?|pptx?|odp)$/.test(ext)) return theme.tokens.status.warning;
  if (/^(docx?|odt|rtf|txt)$/.test(ext)) return theme.tokens.accent.base;
  return theme.tokens.text.secondary;
}

function folderOf(item: MediaFileItem): string {
  if (item.source === 'upload' || item.source === 'transcript') return m.media_folder_uploads();
  if (item.source === 'cited') return attachmentFolder(item.ref).replace(/\/$/, '') || '';
  return '';
}

/** "1,8 MB · ~/Projetos/out · agente · expira em 3 d": o que não se sabe fica fora. */
export function fileDetails(item: MediaFileItem, folder = true): string {
  return [
    item.size != null ? fmtBytes(item.size) : null,
    folder ? folderOf(item) || null : null,
    originLabel(item.origin),
    item.expiresInDays !== undefined ? expiryText(item.expiresInDays).txt : null,
  ].filter(Boolean).join(' · ');
}

// O download em andamento não é estado daqui: é o `progress` do `useDownload`, o mesmo da conversa.
type DownloadState =
  | { kind: 'idle' }
  | { kind: 'done'; file: File }
  | { kind: 'error'; reason: string };

function keptState(key: string, name: string, version: KeptVersion): DownloadState {
  const kept = keptCopy(key, name, version);
  return kept ? { kind: 'done', file: kept } : { kind: 'idle' };
}

/** Baixar → progresso (o anel cancela) → abre; o já baixado abre na hora, sem rede. */
function useKeptFile(item: MediaFileItem, server: Server, sessionName: string, serverId: string) {
  const name = attachmentName(item);
  const keepKey = keptKey(serverId, sessionName, item);
  const size = item.size ?? null;
  const mtime = item.mtime ?? null;
  // O download lê a versão no começo e ao terminar: a medida que chega no meio vale para a cópia gravada.
  const version = useRef<KeptVersion>({ size, mtime });
  useEffect(() => { version.current = { size, mtime }; }, [size, mtime]);
  const [state, setState] = useState<DownloadState>(() => keptState(keepKey, name, { size, mtime }));
  // Data ou tamanho mudou (arquivo regerado, ou a medida chegou depois): a cópia é conferida de novo.
  const shown = `${keepKey}\n${name}\n${size}\n${mtime}`;
  const shownRef = useRef(shown);
  useEffect(() => {
    if (shownRef.current === shown) return;
    shownRef.current = shown;
    setState(keptState(keepKey, name, { size, mtime }));
  }, [shown, keepKey, name, size, mtime]);
  // Sair da tela cancela o download (o `useDownload` aborta ao desmontar).
  const dl = useDownload();
  const alive = useRef(true);
  useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);

  const open = useCallback(async (file: File) => {
    try {
      const r = await deliverLocalFile(file, name);
      if (r === 'opened') haptic.success();
      else if (r === 'empty' || r === 'no-app') {
        haptic.error();
        toast.erro(`${name}: ${r === 'empty' ? m.file_card_empty() : m.file_card_no_app()}`);
      }
    } catch (e) {
      toast.erro(`${name}: ${e instanceof Error ? e.message : String(e)}`);
    }
  }, [name]);

  // O estado só chega no próximo render: dois toques seguidos passariam os dois pela checagem.
  const busy = useRef(false);

  const press = useCallback(() => {
    if (dl.busy) {
      haptic.selection();
      dl.cancel();
      return;
    }
    if (busy.current) return;
    if (state.kind === 'done') {
      haptic.selection();
      busy.current = true;
      void open(state.file).finally(() => { busy.current = false; });
      return;
    }
    // `fileAuthHeader` sempre monta o cabeçalho, mesmo com o token vazio.
    if (!server.token) { toast.erro(m.sessao_expirada()); return; }
    haptic.selection();
    busy.current = true;
    setState({ kind: 'idle' });
    dl.run((o) => downloadKept(keepKey, attachmentUri(item, sessionName, server), attachmentHeaders(item, server), name, () => version.current, o))
      .then(() => {
        if (!alive.current) return;
        // Conferida contra a medida de agora: o arquivo pode ter mudado no servidor durante o download.
        const current = keptCopy(keepKey, name, version.current);
        if (!current) {
          haptic.error();
          setState({ kind: 'error', reason: m.media_changed_during_download() });
          return;
        }
        setState({ kind: 'done', file: current });
        return open(current);
      })
      .catch((e: unknown) => {
        if (!alive.current) return;
        if (isAbort(e)) { setState({ kind: 'idle' }); return; }
        haptic.error();
        // O download do expo-file-system só diz o status no texto; erro do core traz o número.
        const typed = typeof (e as { status?: unknown } | null)?.status === 'number' ? (e as { status: number }).status : null;
        const gone = typed !== null ? typed === 404 : isMissing(e);
        registrarDiag({ evento: 'anexos.entrega_falhou', nivel: 'aviso', tela: 'anexos',
          codigo: typed !== null ? String(typed) : gone ? '404' : e instanceof Error ? e.name : 'erro' });
        setState({ kind: 'error', reason: gone ? m.file_card_missing() : (e instanceof Error && e.message) || m.file_card_failed() });
      })
      .finally(() => { busy.current = false; });
  }, [state, open, keepKey, item, sessionName, server, name, dl.busy, dl.run, dl.cancel]);

  return { state, press, progress: dl.progress, cancel: dl.cancel };
}

function DownloadButton({ state, progress, press, cancel, name, compact = false }: {
  state: DownloadState; progress: DownloadProgress | null; press: () => void; cancel: () => void; name: string; compact?: boolean;
}) {
  const { theme } = useUnistyles();
  if (progress) return <DownloadRing pct={percentOf(progress)} onCancel={cancel} label={m.file_card_cancel({ nome: name })} />;
  const label = state.kind === 'done' ? m.file_card_open() : state.kind === 'error' ? m.media_again() : m.file_card_download();
  const icon = state.kind === 'done' ? 'ExternalLink' : state.kind === 'error' ? 'RotateCw' : 'Download';
  const color = state.kind === 'error' ? theme.tokens.status.error : theme.tokens.text.primary;
  return (
    <Pressable onPress={press} hitSlop={compact ? 0 : 6}
      style={[compact ? styles.iconBtn : styles.mini, { borderColor: theme.tokens.border.default, backgroundColor: superficie(theme) }]}
      accessibilityRole="button" accessibilityLabel={`${label}: ${name}`}>
      <Icon name={icon} size={16} color={color} />
      {compact ? null : <Text style={[styles.miniTxt, { color }]} numberOfLines={1}>{label}</Text>}
    </Pressable>
  );
}

function subLine(state: DownloadState, progress: DownloadProgress | null, item: MediaFileItem): { txt: string; error: boolean } {
  if (progress) return { txt: progressText(progress), error: false };
  if (state.kind === 'error') return { txt: m.media_download_failed({ reason: state.reason }), error: true };
  if (state.kind === 'done') return { txt: [item.size != null ? fmtBytes(item.size) : null, m.media_downloaded(), originLabel(item.origin)].filter(Boolean).join(' · '), error: false };
  return { txt: fileDetails(item), error: false };
}

export const FileRow = memo(function FileRow({ item, server, sessionName, serverId, onOpenDoc }: {
  item: MediaFileItem; server: Server; sessionName: string; serverId: string;
  /** pdf e html abrem no visor do app, como na conversa. */
  onOpenDoc: (item: MediaFileItem) => void;
}) {
  const { theme } = useUnistyles();
  const name = attachmentName(item);
  const { state, press, progress, cancel } = useKeptFile(item, server, sessionName, serverId);
  const ext = extOf(name);
  const sub = subLine(state, progress, item);
  const urgent = state.kind === 'idle' && !progress && item.expiresInDays !== undefined && expiryText(item.expiresInDays).urgent;
  const viewer = item.kind === 'pdf' || item.kind === 'html';
  return (
    <View style={styles.row}>
      <Pressable onPress={() => (viewer ? onOpenDoc(item) : press())} style={styles.rowMain}
        accessibilityRole="button" accessibilityLabel={viewer ? m.anexos_visualizar({ nome: name }) : `${name}, ${sub.txt}`}>
        <View style={[styles.typeBadge, { backgroundColor: badgeColor(ext, theme) }]}>
          <Text style={styles.typeTxt} numberOfLines={1}>{ext.slice(0, 4).toUpperCase()}</Text>
        </View>
        <View style={styles.rowText}>
          <Text style={[styles.rowName, { color: theme.tokens.text.primary }]} numberOfLines={1}>{name}</Text>
          <Text style={[styles.rowSub, { color: sub.error ? theme.tokens.status.error : urgent ? theme.tokens.status.warning : theme.tokens.text.muted }]}
            numberOfLines={1} accessibilityLiveRegion="polite" accessibilityRole={sub.error ? 'alert' : undefined}>
            {sub.txt}
          </Text>
        </View>
      </Pressable>
      <DownloadButton state={state} progress={progress} press={press} cancel={cancel} name={name} />
    </View>
  );
});

// ─── Áudio ─────────────────────────────────────────────────────────────────────────────────────

export const AudioRow = memo(function AudioRow({ item, server, sessionName, serverId, onRetranscribe }: {
  item: MediaFileItem; server: Server; sessionName: string; serverId: string;
  /** Só o áudio enviado volta para a conversa como ditado. */
  onRetranscribe?: (item: MediaFileItem) => void;
}) {
  const { theme } = useUnistyles();
  const name = attachmentName(item);
  const { state, press, progress, cancel } = useKeptFile(item, server, sessionName, serverId);
  return (
    <View style={styles.audioRow}>
      <AudioChip uri={attachmentUri(item, sessionName, server)} headers={attachmentHeaders(item, server)} name={name}
        meta={fileDetails(item, false)} hideShare />
      {onRetranscribe ? (
        <Pressable onPress={() => onRetranscribe(item)} hitSlop={8} style={[styles.mini, { borderColor: theme.tokens.border.default, backgroundColor: superficie(theme) }]}
          accessibilityRole="button" accessibilityLabel={`${m.composer_transcrever_de_novo()}: ${name}`}>
          <Text style={[styles.miniTxt, { color: theme.tokens.accent.base }]} numberOfLines={1}>{m.composer_transcrever_de_novo()}</Text>
        </Pressable>
      ) : null}
      <DownloadButton state={state} progress={progress} press={press} cancel={cancel} name={name} compact />
    </View>
  );
});

const styles = StyleSheet.create((theme) => ({
  fill: {
    width: '100%',
    height: '100%',
  },
  row: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: theme.base.space[2],
    minHeight: 56,
  },
  rowMain: {
    flex: 1,
    minWidth: 0,
    flexDirection: 'row',
    alignItems: 'center',
    gap: theme.base.space[3],
    paddingVertical: theme.base.space[2],
  },
  typeBadge: {
    width: 36,
    height: 36,
    borderRadius: theme.base.radius.sm,
    alignItems: 'center',
    justifyContent: 'center',
  },
  typeTxt: {
    color: theme.ink.onAccent,
    fontSize: theme.base.text.xxxs,
    fontWeight: theme.base.weight.bold,
  },
  rowText: {
    flex: 1,
    minWidth: 0,
    gap: theme.base.space[0.5],
  },
  rowName: {
    fontSize: theme.base.text.sm,
  },
  rowSub: {
    fontSize: theme.base.text.xxs,
    fontVariant: ['tabular-nums'],
  },
  mini: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: theme.base.space[1],
    minHeight: 32,
    paddingHorizontal: theme.base.space[3],
    borderRadius: theme.base.radius.sm,
    borderWidth: 1,
  },
  miniTxt: {
    fontSize: theme.base.text.xs,
    fontWeight: theme.base.weight.medium,
  },
  iconBtn: {
    width: 44,
    height: 44,
    alignItems: 'center',
    justifyContent: 'center',
    borderRadius: theme.base.radius.sm,
    borderWidth: 1,
  },
  audioRow: {
    flexDirection: 'row',
    flexWrap: 'wrap',
    alignItems: 'center',
    gap: theme.base.space[2],
    paddingVertical: theme.base.space[1],
  },
}));
