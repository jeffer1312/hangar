import { useState } from 'react';
import { Pressable, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { BarChart } from 'react-native-gifted-charts';
import { formatarValor } from '@hangar/core';
import type { TabelaLida } from '@hangar/core';
import { PillMenu, type PillMenuItem } from '../features/pills/PillMenu';

interface Props {
  tabela: TabelaLida;
  coluna: number;
  onColuna: (idx: number) => void;
}

export function TableChart({ tabela, coluna, onColuna }: Props) {
  const { theme } = useUnistyles();
  const [open, setOpen] = useState(false);
  const col = tabela.colunas[coluna] ?? tabela.colunas[0];
  const data = tabela.rotulos.map((r, i) => ({
    label: r,
    value: col.valores[i],
    frontColor: theme.tokens.accent.base,
  }));

  const items: PillMenuItem[] = tabela.colunas.map((c, idx) => ({
    label: c.titulo,
    selected: idx === coluna,
  }));

  const handleSelect = (it: PillMenuItem) => {
    const idx = tabela.colunas.findIndex((c) => c.titulo === it.label);
    if (idx >= 0) onColuna(idx);
    setOpen(false);
  };

  return (
    <View style={styles.wrap}>
      {tabela.colunas.length > 1 ? (
        <Pressable
          onPress={() => setOpen(true)}
          style={[styles.selector, { borderColor: theme.tokens.border.subtle, backgroundColor: theme.tokens.bg.surface }]}
          accessibilityRole="button"
        >
          <Text style={[styles.selectorText, { color: theme.tokens.text.primary }]} numberOfLines={1}>
            {col.titulo}
          </Text>
          <Text style={[styles.selectorArrow, { color: theme.tokens.text.muted }]}>▾</Text>
        </Pressable>
      ) : null}
      <BarChart
        key={col.titulo}
        data={data}
        barWidth={28}
        frontColor={theme.tokens.accent.base}
        formatYLabel={(label: string) => formatarValor(Number(label))}
        yAxisTextStyle={{ color: theme.tokens.text.muted, fontSize: 10 }}
        xAxisLabelTextStyle={{ color: theme.tokens.text.muted, fontSize: 10, width: 60 }}
        yAxisLabelWidth={56}
        yAxisThickness={1}
        xAxisThickness={1}
        yAxisColor={theme.tokens.border.subtle}
        xAxisColor={theme.tokens.border.subtle}
        hideRules={false}
        rulesColor={theme.tokens.border.subtle}
        noOfSections={4}
        isAnimated
        spacing={16}
        initialSpacing={12}
      />
      <PillMenu open={open} onClose={() => setOpen(false)} items={items} onSelect={handleSelect} />
    </View>
  );
}

const styles = StyleSheet.create((theme) => ({
  wrap: {
    gap: theme.base.space[2],
    paddingVertical: theme.base.space[1],
  },
  selector: {
    alignSelf: 'flex-start',
    flexDirection: 'row',
    alignItems: 'center',
    gap: 6,
    paddingHorizontal: theme.base.space[2],
    paddingVertical: theme.base.space[1],
    borderRadius: theme.base.radius.full,
    borderWidth: 1,
  },
  selectorText: {
    fontSize: theme.base.text.sm,
    fontWeight: '500',
  },
  selectorArrow: {
    fontSize: 10,
  },
}));
