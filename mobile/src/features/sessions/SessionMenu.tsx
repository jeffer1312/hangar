import type { ReactNode } from 'react';
import { Pressable, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { Sheet } from '../../ui/Sheet';
import { Icon, type IconName } from '../../ui/Icon';
import * as m from '../../paraglide/messages';

export interface Props {
  children: ReactNode;
  // Controlado: quem detecta o toque longo é o gesto da linha (`SessionRow`), que precisa ser o
  // mesmo reconhecedor que convive com o arrasto do swipe.
  aberto: boolean;
  onFechar: () => void;
  temCwd: boolean;
  onRenomear: () => void;
  onGit: () => void;
  onLoop: () => void;
  onExcluir: () => void;
}

// Variante padrão (iOS e o que não for Android): folha de ações do próprio app.
// O Android tem a sua em `SessionMenu.android.tsx`, com o menu nativo do Compose.
export function SessionMenu({ children, aberto, onFechar, temCwd, onRenomear, onGit, onLoop, onExcluir }: Props) {
  const { theme } = useUnistyles();

  const item = (icone: IconName, rotulo: string, acao: () => void, destrutivo?: boolean) => (
    <Pressable
      onPress={() => { onFechar(); acao(); }}
      style={styles.item}
      accessibilityRole="button"
      accessibilityLabel={rotulo}
    >
      <Icon name={icone} size={18} color={destrutivo ? theme.tokens.status.error : theme.tokens.text.secondary} />
      <Text style={[styles.rotulo, { color: destrutivo ? theme.tokens.status.error : theme.tokens.text.primary }]}>{rotulo}</Text>
    </Pressable>
  );

  return (
    <View>
      {children}
      <Sheet open={aberto} sizes={['auto']} onDismiss={onFechar}>
        <View style={styles.inner}>
          {item('PenLine', m.sessao_renomear(), onRenomear)}
          {temCwd ? item('GitBranch', 'Git', onGit) : null}
          {item('RefreshCw', m.loop_titulo(), onLoop)}
          {item('Trash2', m.sessao_excluir_curto(), onExcluir, true)}
        </View>
      </Sheet>
    </View>
  );
}

const styles = StyleSheet.create((theme) => ({
  inner: { padding: theme.base.space[3], gap: theme.base.space[1], paddingBottom: theme.base.space[5] },
  item: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[3], minHeight: 52, paddingHorizontal: theme.base.space[2], borderRadius: theme.base.radius.md },
  rotulo: { fontSize: theme.base.text.base },
}));
