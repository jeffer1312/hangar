import { useEffect, useState } from 'react';
import { Pressable, Text } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { estilosDitado } from '@hangar/core';
import * as m from '../../paraglide/messages';
import { useDitadoEstiloStore } from './ditadoEstiloStore';
import { PillMenu } from '../pills/PillMenu';

export function EstiloPill() {
  const { theme } = useUnistyles();
  const valor = useDitadoEstiloStore((s) => s.valor);
  const revalidar = useDitadoEstiloStore((s) => s.revalidar);
  const trocar = useDitadoEstiloStore((s) => s.trocar);

  const [open, setOpen] = useState(false);
  const [erro, setErro] = useState<string | null>(null);
  const [aplicando, setAplicando] = useState<string | null>(null);

  // carrega na montagem
  useEffect(() => {
    void useDitadoEstiloStore.getState().carregar();
  }, []);

  const lista = estilosDitado();
  const atual = lista.find((e) => e.valor === valor);
  const label = atual?.rotulo ?? m.ditado_estilo_prosa();

  const items = lista.map((e) => ({
    label: e.rotulo,
    hint: e.hint,
    selected: e.valor === valor,
  }));

  const handleOpen = () => {
    setErro(null);
    setAplicando(null);
    void revalidar();
    setOpen(true);
  };

  const handleSelect = async (item: { label: string }) => {
    const hit = lista.find((e) => e.rotulo === item.label);
    if (!hit || aplicando) return;
    setAplicando(hit.valor);
    setErro(null);
    try {
      await trocar(hit.valor);
      setOpen(false);
    } catch (e) {
      setErro(e instanceof Error ? e.message : 'falha');
    } finally {
      setAplicando(null);
    }
  };

  return (
    <>
      <Pressable
        onPress={handleOpen}
        style={[
          styles.pill,
          { backgroundColor: theme.tokens.bg.elevated, borderColor: theme.tokens.border.subtle },
        ]}
        accessibilityRole="button"
        accessibilityLabel={m.ditado_estilo_titulo()}
      >
        <Text style={[styles.text, { color: theme.tokens.text.primary }]} numberOfLines={1}>
          {label}
        </Text>
      </Pressable>
      <PillMenu
        open={open}
        onClose={() => setOpen(false)}
        items={items.map((it) => ({
          ...it,
          label: it.label + (aplicando && lista.find((x) => x.valor === aplicando)?.rotulo === it.label ? ' …' : ''),
        }))}
        onSelect={handleSelect}
        error={erro}
        title={m.ditado_estilo_titulo()}
      />
    </>
  );
}

const styles = StyleSheet.create((theme) => ({
  pill: {
    borderWidth: 1,
    borderRadius: theme.base.radius.full,
    paddingHorizontal: theme.base.space[2],
    paddingVertical: 6,
    minHeight: 32,
    justifyContent: 'center',
    maxWidth: 140,
  },
  text: {
    fontSize: theme.base.text.xs,
    fontWeight: '600',
  },
}));
