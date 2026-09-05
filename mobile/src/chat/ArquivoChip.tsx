import { Pressable, Text } from 'react-native';
import { SvgXml } from 'react-native-svg';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { svgIcone } from '@hangar/core';
import { superficie } from '../theme/superficie';

export function ArquivoChip({ caminho, onPress }: { caminho: string; onPress: () => void }) {
  const { theme } = useUnistyles();
  const nome = caminho.split('/').pop() ?? caminho;
  return (
    <Pressable onPress={onPress} style={styles.chip} accessibilityRole="link" accessibilityLabel={caminho}>
      <SvgXml xml={svgIcone(nome, false)} width={14} height={14} />
      <Text style={[styles.txt, { color: theme.tokens.text.primary }]} numberOfLines={1}>
        {nome}
      </Text>
    </Pressable>
  );
}

const styles = StyleSheet.create((theme) => ({
  chip: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 5,
    paddingHorizontal: 8,
    paddingVertical: 3,
    borderRadius: theme.base.radius.full,
    backgroundColor: superficie(theme),
    alignSelf: 'flex-start',
    maxWidth: 220,
  },
  txt: { fontSize: theme.base.text.xs, fontFamily: theme.base.fontMono, flexShrink: 1 },
}));
