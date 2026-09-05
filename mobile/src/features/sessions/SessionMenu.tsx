import { Pressable, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import type { AggSession } from '@hangar/core';
import { Sheet } from '../../ui/Sheet';
import { Icon, type IconName } from '../../ui/Icon';
import * as m from '../../paraglide/messages';

interface Props {
  sessao: AggSession | null;
  onFechar: () => void;
  onRenomear: (s: AggSession) => void;
  onGit: (s: AggSession) => void;
  onLoop: (s: AggSession) => void;
  onExcluir: (s: AggSession) => void;
}

// Folha de ações do toque longo. NÃO é o menu nativo do `@react-native-menu/menu`: medido no
// emulador (RN 0.86, New Arch), o `shouldOpenOnLongPress` dele só dispara em algumas linhas, e o
// `show()` por ref é no-op no Android — a implementação usa o UIManager antigo. Sem isto, renomear
// e Loop ficariam inalcançáveis na maioria das linhas.
export function SessionMenu({ sessao, onFechar, onRenomear, onGit, onLoop, onExcluir }: Props) {
  const { theme } = useUnistyles();
  const s = sessao;

  const item = (icone: IconName, rotulo: string, acao: (x: AggSession) => void, destrutivo?: boolean) => (
    <Pressable
      onPress={() => { onFechar(); if (s) acao(s); }}
      style={styles.item}
      accessibilityRole="button"
      accessibilityLabel={rotulo}
    >
      <Icon name={icone} size={18} color={destrutivo ? theme.tokens.status.error : theme.tokens.text.secondary} />
      <Text style={[styles.rotulo, { color: destrutivo ? theme.tokens.status.error : theme.tokens.text.primary }]}>{rotulo}</Text>
    </Pressable>
  );

  return (
    <Sheet open={!!s} sizes={['auto']} onDismiss={onFechar}>
      <View style={styles.inner}>
        <Text style={[styles.titulo, { color: theme.tokens.text.muted }]} numberOfLines={1}>{s?.name ?? ''}</Text>
        {item('PenLine', m.sessao_renomear(), onRenomear)}
        {s?.cwd ? item('GitBranch', 'Git', onGit) : null}
        {item('RefreshCw', m.loop_titulo(), onLoop)}
        {item('Trash2', m.sessao_excluir_curto(), onExcluir, true)}
      </View>
    </Sheet>
  );
}

const styles = StyleSheet.create((theme) => ({
  inner: { padding: theme.base.space[3], gap: theme.base.space[1], paddingBottom: theme.base.space[5] },
  titulo: { fontSize: theme.base.text.xs, fontWeight: '600', paddingHorizontal: theme.base.space[2], marginBottom: theme.base.space[1] },
  item: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[3], minHeight: 52, paddingHorizontal: theme.base.space[2], borderRadius: theme.base.radius.md },
  rotulo: { fontSize: theme.base.text.base },
}));
