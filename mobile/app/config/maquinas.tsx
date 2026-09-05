import { Alert, Pressable, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { useRouter } from 'expo-router';
import { getConfigForServer, type Server } from '@hangar/core';
import { Pagina } from '../../src/features/config/Pagina';
import { Linha } from '../../src/features/config/Linha';
import { StateDot } from '../../src/ui/StateDot';
import { Icon } from '../../src/ui/Icon';
import { toast } from '../../src/ui/Toast';
import { useServers } from '../../src/stores/servers';
import * as m from '../../src/paraglide/messages';

export default function Maquinas() {
  const router = useRouter();
  const { theme } = useUnistyles();
  const servers = useServers((s) => s.servers);
  const activeId = useServers((s) => s.activeId);

  const testar = (s: Server) => {
    getConfigForServer(s)
      .then(() => toast.ok(m.config_maquinas_conexao_ok()))
      .catch((e: unknown) => toast.erro(e instanceof Error ? e.message : m.erro_desconhecido()));
  };

  // Remover é sempre com confirmação: a lista pode ter uma máquina só, e sem ela o app volta pro login.
  const remover = (s: Server) =>
    Alert.alert(m.config_servidores_remover({ nome: s.label }), s.baseUrl, [
      { text: m.comum_cancelar(), style: 'cancel' },
      { text: m.lista_remover(), style: 'destructive', onPress: () => useServers.getState().remove(s.id) },
    ]);

  return (
    <Pagina>
      <Text style={styles.intro}>{m.maquinas_intro()}</Text>
      {servers.length === 0 ? <Text style={styles.vazio}>{m.maquinas_vazio()}</Text> : null}
      {servers.map((s) => (
        <Linha
          key={s.id}
          titulo={s.label}
          descricao={s.baseUrl}
          onPress={() => useServers.getState().setActive(s.id)}
          onLongPress={() => remover(s)}
          direita={
            <View style={styles.direita}>
              {/* Ponto só na ativa: as outras não estão erradas, estão paradas — um ponto
                  vermelho ali leria como máquina fora do ar, que ninguém mediu. */}
              {s.id === activeId ? <StateDot state="idle" /> : null}
              <Pressable
                onPress={() => testar(s)}
                accessibilityRole="button"
                accessibilityLabel={m.config_maquinas_testar()}
                hitSlop={8}
                style={styles.testar}
              >
                <Icon name="Plug" size={16} color={theme.tokens.accent.base} />
                <Text style={[styles.testarTxt, { color: theme.tokens.accent.base }]}>
                  {m.config_maquinas_testar()}
                </Text>
              </Pressable>
            </View>
          }
        />
      ))}
      <Linha
        icon="Plus"
        titulo={m.sessao_adicionar_servidor()}
        onPress={() => router.push('/login' as never)}
      />
    </Pagina>
  );
}

const styles = StyleSheet.create((theme) => ({
  intro: { fontSize: theme.base.text.xs, color: theme.tokens.text.muted, paddingHorizontal: theme.base.space[1] },
  vazio: { fontSize: theme.base.text.sm, color: theme.tokens.text.secondary, paddingHorizontal: theme.base.space[1] },
  direita: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2] },
  testar: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[1] },
  testarTxt: { fontSize: theme.base.text.xs, fontWeight: '600' },
}));
