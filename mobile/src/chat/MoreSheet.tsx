import { Modal, Pressable, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { Ionicons } from '@expo/vector-icons';
import { useRouter } from 'expo-router';
import { Glass } from '../ui/Glass';
import * as m from '../paraglide/messages';

interface Props {
  open: boolean;
  onClose: () => void;
  serverId: string;
  name: string;
}

type Item = {
  icon: keyof typeof Ionicons.glyphMap;
  label: string;
  sub?: string;
  route: string;
};

export function MoreSheet({ open, onClose, serverId, name }: Props) {
  const { theme } = useUnistyles();
  const router = useRouter();

  const items: Item[] = [
    { icon: 'help-circle-outline', label: m.askq_sua_resposta(), route: 'ask' },
    { icon: 'pulse-outline', label: m.ctx_atividade(), sub: m.more_tarefas_agentes(), route: 'activity' },
    { icon: 'repeat-outline', label: m.loop_titulo(), sub: m.loop_objetivo(), route: 'loop' },
    { icon: 'people-outline', label: m.par_titulo(), sub: m.ctx_grupo(), route: 'pair' },
    { icon: 'folder-outline', label: m.arq_aba(), sub: m.ctx_repositorio(), route: 'files' },
    { icon: 'terminal-outline', label: m.term_titulo(), sub: m.ctx_terminal(), route: 'terminal' },
    { icon: 'attach-outline', label: m.ctx_anexos(), sub: m.more_fotos_videos_arquivos(), route: 'attachments' },
    { icon: 'speedometer-outline', label: m.codex_limites_titulo(), sub: m.ctx_limites(), route: 'codex-limits' },
  ];

  const go = (route: string) => {
    onClose();
    router.push(`/s/${serverId}/${name}/${route}` as never);
  };

  if (!open) return null;

  return (
    <Modal visible={open} transparent animationType="fade" onRequestClose={onClose}>
      <Pressable style={styles.backdrop} onPress={onClose}>
        <Glass variant="modal" style={styles.sheet}>
          <Pressable onPress={() => {}} style={styles.inner}>
            <Text style={[styles.title, { color: theme.tokens.text.primary }]}>{m.navbar_mais_acoes()}</Text>
            {items.map((it) => (
              <Pressable
                key={it.route}
                onPress={() => go(it.route)}
                style={styles.item}
                accessibilityRole="button"
                accessibilityLabel={it.label}
              >
                <View style={[styles.ico, { backgroundColor: theme.tokens.bg.elevated }]}>
                  <Ionicons name={it.icon} size={20} color={theme.tokens.text.secondary} />
                </View>
                <View style={styles.txt}>
                  <Text style={[styles.label, { color: theme.tokens.text.primary }]}>{it.label}</Text>
                  {it.sub ? <Text style={[styles.sub, { color: theme.tokens.text.muted }]} numberOfLines={1}>{it.sub}</Text> : null}
                </View>
                <Ionicons name="chevron-forward" size={16} color={theme.tokens.text.muted} />
              </Pressable>
            ))}
          </Pressable>
        </Glass>
      </Pressable>
    </Modal>
  );
}

const styles = StyleSheet.create((theme) => ({
  backdrop: {
    flex: 1,
    backgroundColor: 'rgba(0,0,0,0.35)',
    justifyContent: 'flex-end',
    padding: theme.base.space[4],
  },
  sheet: {
    padding: theme.base.space[3],
    maxHeight: '80%',
  },
  inner: {
    gap: theme.base.space[1],
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
