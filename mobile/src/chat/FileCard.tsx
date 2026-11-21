import { memo, useCallback, useRef, useState } from 'react';
import { Platform, Pressable, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { haptic } from '../ui/haptics';
import { SvgXml } from 'react-native-svg';
import { fmtBytes, svgIcone } from '@hangar/core';
import { Icon } from '../ui/Icon';
import { superficie } from '../theme/superficie';
import { DownloadRing } from '../features/attachments/DownloadRing';
import { percentOf, useDownload } from '../features/attachments/useDownload';
import type { DownloadOptions } from '../features/attachments/mediaCache';
import * as m from '../paraglide/messages';

type Status = 'idle' | 'done' | 'opened' | 'empty' | 'missing' | 'failed' | 'no-app';

const statusText: Record<Status, ((size: string) => string) | null> = {
  idle: null,
  done: (size) => (size ? m.file_card_downloaded({ size }) : m.file_card_saved()),
  opened: () => m.file_card_opened(),
  empty: () => m.file_card_empty(),
  missing: () => m.file_card_missing(),
  failed: () => m.file_card_failed(),
  'no-app': () => m.file_card_no_app(),
};

// Sob demanda: a bolha do usuário também desenha o card, e o módulo nativo de arquivos só carrega no toque.
const cache = () => import('../features/attachments/mediaCache');

// Documento citado na conversa (docx, planilha, zip, csv…): o card do nativo, com a ação que o
// celular tem para o tipo. iOS entrega pela folha do sistema (que tem "Salvar em Arquivos"); Android
// abre no app que lê o arquivo e tem o compartilhar ao lado, que também serve quando nenhum app abre.
export const FileCard = memo(function FileCard({ uri, headers, name }: {
  uri: string;
  headers?: Record<string, string>;
  name: string;
}) {
  const { theme } = useUnistyles();
  const [status, setStatus] = useState<Status>('idle');
  const [size, setSize] = useState<number | null>(null);
  const dl = useDownload();
  const lastSize = useRef<number | null>(null);
  const ios = Platform.OS === 'ios';

  const go = useCallback(async (action: 'deliver' | 'share') => {
    if (dl.busy) return;
    haptic.selection();
    try {
      const mc = await cache();
      const r = await dl.run((o: DownloadOptions) => {
        const opts: DownloadOptions = { ...o, onProgress: (p) => { lastSize.current = p.total ?? p.written; o.onProgress?.(p); } };
        return action === 'share'
          ? mc.shareFile(uri, headers, name, opts).then(() => 'shared' as const)
          : mc.deliverFile(uri, headers, name, opts);
      });
      setSize(lastSize.current);
      setStatus(r === 'shared' ? 'done' : r);
      if (r === 'empty' || r === 'no-app') haptic.error();
      else if (r === 'opened') haptic.success();
    } catch (e) {
      // Cancelado pelo anel: volta ao que era, sem erro.
      if (e instanceof Error && e.name === 'AbortError') return;
      // O download recusa resposta fora de 2xx com "status 404" (iOS), "status: 404" ou "HTTP 404" (Android).
      setStatus(/(status:?|HTTP) 404\b/.test(e instanceof Error ? e.message : String(e)) ? 'missing' : 'failed');
      haptic.error();
    }
  }, [uri, headers, name, dl]);

  const ext = name.includes('.') ? name.split('.').pop()!.toUpperCase() : '';
  const pct = percentOf(dl.progress);
  const statusLine = dl.progress
    ? pct != null && dl.progress.total
      ? m.file_card_progress({ pct: String(pct), size: fmtBytes(dl.progress.total) })
      : dl.progress.written > 0
        ? m.file_card_progress_unknown({ size: fmtBytes(dl.progress.written) })
        : m.file_card_downloading()
    : statusText[status]?.(size != null ? fmtBytes(size) : '');
  const erro = !dl.busy && (status === 'missing' || status === 'failed' || status === 'empty' || status === 'no-app');
  // Arquivo vazio não melhora tentando de novo.
  const retry = status === 'missing' || status === 'failed';
  const label = retry ? m.file_card_retry() : ios ? m.file_card_download() : m.file_card_open();
  return (
    <View style={[styles.card, { backgroundColor: superficie(theme), borderColor: theme.tokens.border.subtle }]}>
      <View style={[styles.iconBox, { backgroundColor: theme.tokens.accent.dim }]}>
        <SvgXml xml={svgIcone(name, false)} width={22} height={22} />
      </View>
      <View style={styles.texts}>
        <Text style={[styles.name, { color: theme.tokens.text.primary }]} numberOfLines={1}>{name}</Text>
        <Text
          style={[styles.meta, { color: erro ? theme.tokens.status.error : theme.tokens.text.muted }]}
          numberOfLines={2}
          accessibilityLiveRegion="polite"
          accessibilityRole={erro ? 'alert' : undefined}
        >
          {[ext, statusLine].filter(Boolean).join(' · ')}
        </Text>
      </View>
      {dl.busy ? (
        <DownloadRing pct={pct} onCancel={dl.cancel} label={m.file_card_cancel({ nome: name })} />
      ) : (
        <>
          {!ios && !retry ? (
            <Pressable
              onPress={() => void go('share')}
              style={[styles.iconAction, { borderColor: theme.tokens.border.default }]}
              accessibilityRole="button"
              accessibilityLabel={m.anexos_compartilhar({ nome: name })}
            >
              <Icon name="Share2" size={16} color={theme.tokens.text.secondary} />
            </Pressable>
          ) : null}
          <Pressable
            onPress={() => void go('deliver')}
            style={[styles.action, { borderColor: theme.tokens.border.default }]}
            accessibilityRole="button"
            accessibilityLabel={`${label} ${name}`}
          >
            <Icon name={retry ? 'RotateCw' : ios ? 'Download' : 'ExternalLink'} size={16} color={theme.tokens.text.secondary} />
            <Text style={[styles.actionText, { color: theme.tokens.text.primary }]}>{label}</Text>
          </Pressable>
        </>
      )}
    </View>
  );
});

const styles = StyleSheet.create((theme) => ({
  card: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: theme.base.space[3],
    padding: theme.base.space[2],
    paddingLeft: theme.base.space[3],
    borderRadius: theme.base.radius.md,
    borderWidth: 1,
    width: '100%',
    maxWidth: 420,
  },
  iconBox: {
    width: 36,
    height: 36,
    borderRadius: theme.base.radius.sm,
    alignItems: 'center',
    justifyContent: 'center',
  },
  texts: {
    flex: 1,
    minWidth: 0,
    gap: theme.base.space[0.5],
  },
  name: {
    fontSize: theme.base.text.sm,
    fontWeight: theme.base.weight.semibold,
  },
  meta: {
    fontSize: theme.base.text.xs,
  },
  action: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: theme.base.space[1],
    minHeight: 44,
    minWidth: 44,
    paddingHorizontal: theme.base.space[3],
    borderRadius: theme.base.radius.md,
    borderWidth: 1,
    justifyContent: 'center',
  },
  iconAction: {
    width: 44,
    height: 44,
    borderRadius: theme.base.radius.md,
    borderWidth: 1,
    alignItems: 'center',
    justifyContent: 'center',
  },
  actionText: {
    fontSize: theme.base.text.sm,
    fontWeight: theme.base.weight.semibold,
  },
}));
