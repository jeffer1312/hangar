import { Pressable, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { useRouter } from 'expo-router';
import { Sheet } from '../../ui/Sheet';
import { Icon } from '../../ui/Icon';
import { useServers } from '../../stores/servers';
import * as m from '../../paraglide/messages';

// Trocar a máquina ativa (é ela quem responde às ações da lista) e chegar no pareamento.
// Controlada por `open`, não por ref: é o `Sheet` que sabe reclamar quando present()/dismiss()
// falham — pela ref, o toque que não abria nada sumia calado.
export function ServerSheet({ open, onFechar }: { open: boolean; onFechar: () => void }) {
  const { theme } = useUnistyles();
  const router = useRouter();
  const servers = useServers((s) => s.servers);
  const activeId = useServers((s) => s.activeId);

  return (
    <Sheet open={open} onDismiss={onFechar} sizes={['auto']}>
      <View style={styles.inner}>
        <Text style={[styles.title, { color: theme.tokens.text.primary }]}>{m.maquinas_este_aparelho()}</Text>
        {servers.map((s) => (
          <Pressable
            key={s.id}
            onPress={() => { useServers.getState().setActive(s.id); onFechar(); }}
            style={styles.item}
            accessibilityRole="button"
            accessibilityLabel={s.label}
            accessibilityState={{ selected: s.id === activeId }}
          >
            <Text style={[styles.nome, { color: theme.tokens.text.primary, fontWeight: s.id === activeId ? '700' : '500' }]} numberOfLines={1}>
              {s.label}
            </Text>
            {s.id === activeId ? <Icon name="Check" size={16} color={theme.tokens.accent.base} /> : null}
          </Pressable>
        ))}
        <Pressable
          onPress={() => { onFechar(); router.push('/login' as never); }}
          style={styles.item}
          accessibilityRole="button"
          accessibilityLabel={m.sessao_adicionar_servidor()}
        >
          <Icon name="Plus" size={16} color={theme.tokens.accent.base} />
          <Text style={[styles.nome, { color: theme.tokens.accent.base }]}>{m.sessao_adicionar_servidor()}</Text>
        </Pressable>
      </View>
    </Sheet>
  );
}

const styles = StyleSheet.create((theme) => ({
  inner: { padding: theme.base.space[3], gap: theme.base.space[1] },
  title: { fontSize: theme.base.text.base, fontWeight: '600', marginBottom: theme.base.space[2] },
  item: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[3], minHeight: 48, paddingHorizontal: theme.base.space[2], borderRadius: theme.base.radius.md },
  nome: { flex: 1, fontSize: theme.base.text.base },
}));
