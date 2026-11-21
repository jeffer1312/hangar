import { memo, useCallback, useEffect, useRef, useState } from 'react';
import { ActivityIndicator, Platform, Pressable, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { haptic } from '../ui/haptics';
import { SvgXml } from 'react-native-svg';
import { svgIcone } from '@hangar/core';
import { Icon } from '../ui/Icon';
import { superficie } from '../theme/superficie';
import * as m from '../paraglide/messages';

type Status = 'idle' | 'loading' | 'opened' | 'empty' | 'missing' | 'failed' | 'no-app';

const statusText: Record<Status, (() => string) | null> = {
  idle: null,
  loading: m.file_card_downloading,
  opened: m.file_card_opened,
  empty: m.file_card_empty,
  missing: m.file_card_missing,
  failed: m.file_card_failed,
  'no-app': m.file_card_no_app,
};

// Documento citado na conversa (docx, planilha, zip, csv, txt…): o card do nativo, com a ação que o
// celular tem para o tipo. iOS baixa pela folha do sistema; Android abre no app que lê o arquivo.
export const FileCard = memo(function FileCard({ uri, headers, name }: {
  uri: string;
  headers?: Record<string, string>;
  name: string;
}) {
  const { theme } = useUnistyles();
  const [status, setStatus] = useState<Status>('idle');
  const alive = useRef(true);
  useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);
  const ios = Platform.OS === 'ios';
  const run = useCallback(async () => {
    haptic.selection();
    setStatus('loading');
    try {
      // Sob demanda: a bolha do usuário também desenha o card, e o módulo nativo de arquivos só carrega no toque.
      const { deliverFile } = await import('../features/attachments/mediaCache');
      const r = await deliverFile(uri, headers, name);
      if (!alive.current) return;
      setStatus(r === 'cancelled' || r === 'shared' ? 'idle' : r);
      if (r === 'empty' || r === 'no-app') haptic.error();
      else if (r === 'opened') haptic.success();
    } catch (e) {
      if (!alive.current) return;
      // O download recusa resposta fora de 2xx com "status 404" (iOS) ou "status: 404" (Android).
      setStatus(/status:? 404\b/.test(e instanceof Error ? e.message : String(e)) ? 'missing' : 'failed');
      haptic.error();
    }
  }, [uri, headers, name]);

  const ext = name.includes('.') ? name.split('.').pop()!.toUpperCase() : '';
  const statusLine = statusText[status]?.();
  const erro = status === 'missing' || status === 'failed' || status === 'empty' || status === 'no-app';
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
          numberOfLines={1}
          accessibilityLiveRegion="polite"
          accessibilityRole={erro ? 'alert' : undefined}
        >
          {[ext, statusLine].filter(Boolean).join(' · ')}
        </Text>
      </View>
      <Pressable
        onPress={run}
        disabled={status === 'loading'}
        style={[styles.action, { borderColor: theme.tokens.border.default }]}
        accessibilityRole="button"
        accessibilityLabel={`${label} ${name}`}
        accessibilityState={{ busy: status === 'loading' }}
      >
        {status === 'loading'
          ? <ActivityIndicator size="small" color={theme.tokens.text.secondary} />
          : <Icon name={retry ? 'RotateCw' : ios ? 'Download' : 'ExternalLink'} size={16} color={theme.tokens.text.secondary} />}
        <Text style={[styles.actionText, { color: theme.tokens.text.primary }]}>{label}</Text>
      </Pressable>
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
  actionText: {
    fontSize: theme.base.text.sm,
    fontWeight: theme.base.weight.semibold,
  },
}));
