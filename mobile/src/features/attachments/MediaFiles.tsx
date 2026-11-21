import { memo, useCallback, useEffect, useRef, useState } from 'react';
import { ActivityIndicator, Pressable, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import type { UnistylesThemes } from 'react-native-unistyles';
import { Image } from 'expo-image';
import Svg, { Circle, Path } from 'react-native-svg';
import type { File } from 'expo-file-system';
import { attachmentFolder, fmtBytes, registrarDiag, type ConversationAttachment, type Server } from '@hangar/core';
import * as m from '../../paraglide/messages';
import { Icon } from '../../ui/Icon';
import { toast } from '../../ui/Toast';
import { haptic } from '../../ui/haptics';
import { superficie } from '../../theme/superficie';
import { AudioChip } from '../../chat/AudioChip';
import { attachmentHeaders, attachmentName, attachmentThumbUri, attachmentUri, keptKey } from './attachmentUrls';
import { deliverLocalFile, downloadKept, keptCopy } from './mediaCache';
import { expiryText, originLabel } from './gallery';

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
  const { theme } = useUnistyles();
  const name = attachmentName(item);
  const thumb = attachmentThumbUri(item, sessionName, server, size) ?? attachmentUri(item, sessionName, server);
  const [failed, setFailed] = useState<string | null>(null);
  const broken = failed === thumb;
  return (
    <Pressable
      onPress={() => onOpen(item)}
      style={[styles.cell, { width: size, height: size }]}
      accessibilityRole="imagebutton"
      accessibilityLabel={item.kind === 'video' ? m.anexos_tocar({ nome: name }) : m.anexos_ver({ n: name })}
    >
      {item.kind === 'image' && !broken ? (
        <Image source={{ uri: thumb, headers: attachmentHeaders(item, server) }} style={styles.fill} contentFit="cover" transition={150}
          onError={() => setFailed(thumb)} />
      ) : (
        <View style={[styles.fill, styles.cellPlaceholder]}>
          <Icon name={broken ? 'ImageOff' : 'Play'} size={22} color={theme.ink.onAccent} />
        </View>
      )}
      <View style={styles.badge}>
        <Text style={styles.badgeTxt} numberOfLines={1}>{originLabel(item.origin)}</Text>
      </View>
    </Pressable>
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

type DownloadState =
  | { kind: 'idle' }
  | { kind: 'loading'; progress: number | null }
  | { kind: 'done'; file: File }
  | { kind: 'error'; reason: string };

const missing = (e: unknown) => /status:? 404\b|HTTP 404\b/.test(e instanceof Error ? e.message : String(e));
const aborted = (e: unknown) => e instanceof Error && (e.name === 'AbortError' || /cancel/i.test(e.message));

function keptState(key: string, name: string): DownloadState {
  const kept = keptCopy(key, name);
  return kept ? { kind: 'done', file: kept } : { kind: 'idle' };
}

/** Baixar → progresso (tocar cancela) → abre; o já baixado abre na hora, sem rede. */
function useDownload(item: MediaFileItem, server: Server, sessionName: string, serverId: string) {
  const name = attachmentName(item);
  const keepKey = keptKey(serverId, sessionName, item);
  const [state, setState] = useState<DownloadState>(() => keptState(keepKey, name));
  // Data ou tamanho mudou (arquivo regerado, ou a medida chegou depois): vale a cópia desta versão.
  const shownKey = useRef(keepKey);
  useEffect(() => {
    if (shownKey.current === keepKey) return;
    shownKey.current = keepKey;
    setState((s) => (s.kind === 'loading' ? s : keptState(keepKey, name)));
  }, [keepKey, name]);
  const abort = useRef<AbortController | null>(null);
  const alive = useRef(true);
  useEffect(() => { alive.current = true; return () => { alive.current = false; abort.current?.abort(); }; }, []);

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
    if (state.kind === 'loading') {
      haptic.selection();
      abort.current?.abort();
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
    const ctrl = new AbortController();
    abort.current = ctrl;
    setState({ kind: 'loading', progress: null });
    let last = -1;
    downloadKept(keepKey, attachmentUri(item, sessionName, server), attachmentHeaders(item, server), name, (p) => {
      // Um redesenho por ponto percentual, não por pedaço recebido.
      const pct = p === null ? -1 : Math.floor(p * 100);
      if (pct === last || !alive.current) return;
      last = pct;
      setState({ kind: 'loading', progress: p });
    }, ctrl.signal)
      .then((file) => {
        if (!alive.current) return;
        setState({ kind: 'done', file });
        return open(file);
      })
      .catch((e: unknown) => {
        if (!alive.current) return;
        if (ctrl.signal.aborted || aborted(e)) { setState({ kind: 'idle' }); return; }
        haptic.error();
        // O download do expo-file-system só diz o status no texto; erro do core traz o número.
        const typed = typeof (e as { status?: unknown } | null)?.status === 'number' ? (e as { status: number }).status : null;
        const gone = typed !== null ? typed === 404 : missing(e);
        registrarDiag({ evento: 'anexos.entrega_falhou', nivel: 'aviso', tela: 'anexos',
          codigo: typed !== null ? String(typed) : gone ? '404' : e instanceof Error ? e.name : 'erro' });
        setState({ kind: 'error', reason: gone ? m.file_card_missing() : (e instanceof Error && e.message) || m.file_card_failed() });
      })
      .finally(() => { busy.current = false; });
  }, [state, open, keepKey, item, sessionName, server, name]);

  return { state, press };
}

function ProgressRing({ progress, color, track, mark }: { progress: number; color: string; track: string; mark: string }) {
  const r = 12;
  const c = 2 * Math.PI * r;
  return (
    <Svg width={30} height={30} viewBox="0 0 30 30">
      <Circle cx={15} cy={15} r={r} fill="none" stroke={track} strokeWidth={3} />
      <Circle cx={15} cy={15} r={r} fill="none" stroke={color} strokeWidth={3} strokeLinecap="round"
        strokeDasharray={`${c * progress} ${c}`} transform="rotate(-90 15 15)" />
      <Path d="M11 11l8 8M19 11l-8 8" stroke={mark} strokeWidth={1.6} />
    </Svg>
  );
}

function DownloadButton({ state, press, name, compact = false }: {
  state: DownloadState; press: () => void; name: string; compact?: boolean;
}) {
  const { theme } = useUnistyles();
  if (state.kind === 'loading') {
    return (
      <Pressable onPress={press} style={styles.ringBtn} accessibilityRole="button"
        accessibilityLabel={m.media_cancel_download({ nome: name })}
        accessibilityValue={state.progress !== null ? { min: 0, max: 100, now: Math.round(state.progress * 100) } : undefined}>
        {state.progress === null
          ? <ActivityIndicator color={theme.tokens.accent.base} />
          : <ProgressRing progress={state.progress} color={theme.tokens.accent.base} track={theme.tokens.border.strong} mark={theme.tokens.text.secondary} />}
      </Pressable>
    );
  }
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

function subLine(state: DownloadState, item: MediaFileItem): { txt: string; error: boolean } {
  if (state.kind === 'loading') {
    if (state.progress === null) return { txt: m.file_card_downloading(), error: false };
    const pct = Math.round(state.progress * 100);
    return { txt: item.size != null ? m.media_downloading_pct_of({ pct, total: fmtBytes(item.size) }) : m.media_downloading_pct({ pct }), error: false };
  }
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
  const { state, press } = useDownload(item, server, sessionName, serverId);
  const ext = extOf(name);
  const sub = subLine(state, item);
  const urgent = state.kind === 'idle' && item.expiresInDays !== undefined && expiryText(item.expiresInDays).urgent;
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
      <DownloadButton state={state} press={press} name={name} />
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
  const { state, press } = useDownload(item, server, sessionName, serverId);
  return (
    <View style={styles.audioRow}>
      <AudioChip uri={attachmentUri(item, sessionName, server)} headers={attachmentHeaders(item, server)} name={name}
        meta={fileDetails(item, false)} />
      {onRetranscribe ? (
        <Pressable onPress={() => onRetranscribe(item)} hitSlop={8} style={[styles.mini, { borderColor: theme.tokens.border.default, backgroundColor: superficie(theme) }]}
          accessibilityRole="button" accessibilityLabel={`${m.composer_transcrever_de_novo()}: ${name}`}>
          <Text style={[styles.miniTxt, { color: theme.tokens.accent.base }]} numberOfLines={1}>{m.composer_transcrever_de_novo()}</Text>
        </Pressable>
      ) : null}
      <DownloadButton state={state} press={press} name={name} compact />
    </View>
  );
});

const styles = StyleSheet.create((theme) => ({
  cell: {
    overflow: 'hidden',
    backgroundColor: superficie(theme, 0.8),
  },
  fill: {
    width: '100%',
    height: '100%',
  },
  cellPlaceholder: {
    alignItems: 'center',
    justifyContent: 'center',
    backgroundColor: '#111',
  },
  badge: {
    position: 'absolute',
    left: theme.base.space[1],
    top: theme.base.space[1],
    paddingHorizontal: theme.base.space[1],
    borderRadius: theme.base.radius.xs,
    backgroundColor: theme.ink.scrim,
  },
  badgeTxt: {
    color: theme.ink.onAccent,
    fontSize: theme.base.text.xxxs,
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
  ringBtn: {
    width: 44,
    height: 44,
    alignItems: 'center',
    justifyContent: 'center',
  },
  audioRow: {
    flexDirection: 'row',
    flexWrap: 'wrap',
    alignItems: 'center',
    gap: theme.base.space[2],
    paddingVertical: theme.base.space[1],
  },
}));
