import { Pressable, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { useRouter } from 'expo-router';
import { Sheet } from '../ui/Sheet';
import { Icon, type IconName } from '../ui/Icon';
import * as m from '../paraglide/messages';
import { superficie } from '../theme/superficie';

interface Props {
  open: boolean;
  onClose: () => void;
  serverId: string;
  name: string;
}

type Item = {
  icon: IconName;
  label: string;
  sub?: string;
  route: string;
};

export function MoreSheet({ open, onClose, serverId, name }: Props) {
  const { theme } = useUnistyles();
  const router = useRouter();

  const items: Item[] = [
    { icon: 'CircleHelp', label: m.askq_sua_resposta(), route: 'ask' },
    { icon: 'Activity', label: m.ctx_atividade(), sub: m.more_tarefas_agentes(), route: 'activity' },
    { icon: 'Repeat', label: m.loop_titulo(), sub: m.loop_objetivo(), route: 'loop' },
    { icon: 'Users', label: m.par_titulo(), sub: m.ctx_grupo(), route: 'pair' },
    { icon: 'Folder', label: m.arq_aba(), sub: m.ctx_repositorio(), route: 'files' },
    { icon: 'Terminal', label: m.term_titulo(), sub: m.ctx_terminal(), route: 'terminal' },
    { icon: 'Paperclip', label: m.ctx_anexos(), sub: m.more_fotos_videos_arquivos(), route: 'attachments' },
    { icon: 'Gauge', label: m.codex_limites_titulo(), sub: m.ctx_limites(), route: 'codex-limits' },
    { icon: 'Flag', label: m.bastao_dossie_titulo(), sub: m.bastao_dossie_sub(), route: 'bastao' },
  ];

  const go = (route: string) => {
    onClose();
    router.push(`/s/${serverId}/${name}/${route}` as never);
  };

  return (
    <Sheet open={open} sizes={['auto']} onDismiss={onClose}>
      <View style={styles.inner}>
        <Text style={[styles.title, { color: theme.tokens.text.primary }]}>{m.navbar_mais_acoes()}</Text>
        {items.map((it) => (
          <Pressable
            key={it.route}
            onPress={() => go(it.route)}
            style={styles.item}
            accessibilityRole="button"
            accessibilityLabel={it.label}
          >
            <View style={[styles.ico, { backgroundColor: superficie(theme, 0.8) }]}>
              <Icon name={it.icon} size={20} color={theme.tokens.text.secondary} />
            </View>
            <View style={styles.txt}>
              <Text style={[styles.label, { color: theme.tokens.text.primary }]}>{it.label}</Text>
              {it.sub ? <Text style={[styles.sub, { color: theme.tokens.text.muted }]} numberOfLines={1}>{it.sub}</Text> : null}
            </View>
            <Icon name="ChevronRight" size={16} color={theme.tokens.text.muted} />
          </Pressable>
        ))}
      </View>
    </Sheet>
  );
}

const styles = StyleSheet.create((theme) => ({
  inner: {
    gap: theme.base.space[1],
    padding: theme.base.space[3],
  },
  title: {
    fontSize: theme.base.text.base,
    fontWeight: '600',
    marginBottom: theme.base.space[2],
  },
  item: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: theme.base.space[3],
    minHeight: 56,
    paddingHorizontal: theme.base.space[2],
    borderRadius: theme.base.radius.md,
  },
  ico: {
    width: 36,
    height: 36,
    borderRadius: theme.base.radius.sm,
    alignItems: 'center',
    justifyContent: 'center',
  },
  txt: {
    flex: 1,
    gap: 1,
  },
  label: {
    fontSize: theme.base.text.base,
    fontWeight: '600',
  },
  sub: {
    fontSize: theme.base.text.xs,
  },
}));
