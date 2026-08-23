import { useEffect, useState } from 'react';
import { ActivityIndicator, Pressable, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { useLocalSearchParams } from 'expo-router';
import { useServers } from '../../../../src/stores/servers';
import { filesStore, entriesOf, listaCortadaOf } from '../../../../src/features/files/filesStore';
import { FileTree } from '../../../../src/features/files/FileTree';
import { FileSearchBar } from '../../../../src/features/files/FileSearchBar';
import { FileViewer } from '../../../../src/features/files/FileViewer';
import { FileEditor } from '../../../../src/features/files/FileEditor';
import * as m from '../../../../src/paraglide/messages';

type Aba = 'arvore' | 'busca' | 'arquivo';

export default function FilesSheet() {
  const { theme } = useUnistyles();
  const { server, name, path } = useLocalSearchParams<{ server: string; name: string; path?: string }>();
  const serverId = String(server ?? '');
  const sessionName = String(name ?? '');
  const pathParam = Array.isArray(path) ? path[0] : path;

  const api = filesStore(serverId, sessionName);
  const abertos = api.use((s) => s.abertos);
  const porPasta = api.use((s) => s.porPasta);
  const cortePorPasta = api.use((s) => s.cortePorPasta);
  const selecionado = api.use((s) => s.selecionado);
  const conteudo = api.use((s) => s.conteudo);
  const diff = api.use((s) => s.diff);
  const escopo = api.use((s) => s.escopo);
  const resultados = api.use((s) => s.resultados);
  const erro = api.use((s) => s.erro);
  const soModificados = api.use((s) => s.soModificados);
  const buscaCortada = api.use((s) => s.buscaCortada);
  const loading = api.use((s) => s.loading);

  const [aba, setAba] = useState<Aba>('arvore');
  const [editando, setEditando] = useState(false);
  const [qBusca, setQBusca] = useState('');
  const [modoBusca, setModoBusca] = useState<'names' | 'contents'>('names');
  const ready = useServers((s) => s.ready);
  const [servidorSumiu, setServidorSumiu] = useState(false);

  useEffect(() => {
    if (!ready) return;
    if (!useServers.getState().ensureActive(serverId)) {
      setServidorSumiu(true);
      return;
    }
    setServidorSumiu(false);
    api.retain();
    return () => api.release();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [ready, serverId, sessionName]);

  useEffect(() => {
    if (!ready) return;
    const existe = useServers.getState().servers.some((s) => s.id === serverId);
    if (!existe) setServidorSumiu(true);
  }, [ready, serverId]);

  // abrir direto via ?path=
  useEffect(() => {
    if (pathParam) {
      void api.abrir(pathParam);
      setAba('arquivo');
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [pathParam]);

  // quando seleciona arquivo, vai para aba arquivo
  useEffect(() => {
    if (selecionado) setAba('arquivo');
  }, [selecionado]);

  const entries = entriesOf(porPasta, abertos);
  const listaCortada = listaCortadaOf(cortePorPasta, abertos);

  const handleToggleFiltro = () => {
    api.use.setState({ soModificados: !soModificados });
    void api.recarregar();
  };

  const handleBusca = (q: string, mode: 'names' | 'contents') => {
    setQBusca(q);
    setModoBusca(mode);
    if (q.trim() === '') {
      api.use.setState({ resultados: [] });
      return;
    }
    void api.buscar(q, mode);
  };

  const handlePick = (p: string) => {
    void api.abrir(p);
  };

  const handleTogglePasta = (p: string) => {
    void api.alternarPasta(p);
  };

  const handleSalvar = async (texto: string): Promise<string | null> => {
    if (!selecionado) return 'erro_arq_inexistente';
    const r = await api.salvar(selecionado, texto);
    return r;
  };

  if (!ready) {
    return (
      <View style={[styles.root, { backgroundColor: theme.tokens.bg.base, flex: 1, alignItems: 'center', justifyContent: 'center' }]}>
        <ActivityIndicator color={theme.tokens.text.muted} />
        <Text style={[styles.muted, { color: theme.tokens.text.muted }]}>{m.comum_carregando()}</Text>
      </View>
    );
  }

  if (servidorSumiu) {
    return (
      <View style={[styles.root, { backgroundColor: theme.tokens.bg.base, flex: 1, alignItems: 'center', justifyContent: 'center', gap: 8 }]}>
        <Text style={[styles.erro, { color: theme.tokens.status.error }]} accessibilityRole="alert">
          {m.arq_sessao_encerrada()}
        </Text>
        <Pressable
          onPress={() => {
            // volta — o chamador decide; sem rota ativa, não há onde recarregar
          }}
          accessibilityRole="button"
        >
          <Text style={{ color: theme.tokens.accent.base, fontSize: 12 }}>{m.comum_voltar()}</Text>
        </Pressable>
      </View>
    );
  }

  return (
    <View style={[styles.root, { backgroundColor: theme.tokens.bg.base }]}>
      {/* tabs */}
      <View style={[styles.tabs, { borderBottomColor: theme.tokens.border.subtle }]}>
        <Pressable onPress={() => setAba('arvore')} style={[styles.tab, aba === 'arvore' ? { borderBottomColor: theme.tokens.accent.base } : null]}>
          <Text style={[styles.tabTxt, { color: aba === 'arvore' ? theme.tokens.accent.base : theme.tokens.text.secondary }]}>{m.arq_aba()}</Text>
        </Pressable>
        <Pressable onPress={() => setAba('busca')} style={[styles.tab, aba === 'busca' ? { borderBottomColor: theme.tokens.accent.base } : null]}>
          <Text style={[styles.tabTxt, { color: aba === 'busca' ? theme.tokens.accent.base : theme.tokens.text.secondary }]}>{m.arq_buscar()}</Text>
        </Pressable>
        <Pressable onPress={() => setAba('arquivo')} style={[styles.tab, aba === 'arquivo' ? { borderBottomColor: theme.tokens.accent.base } : null]}>
          <Text style={[styles.tabTxt, { color: aba === 'arquivo' ? theme.tokens.accent.base : theme.tokens.text.secondary }]}>{selecionado ?? m.arq_ver_arquivo()}</Text>
        </Pressable>
      </View>

      {aba === 'arvore' ? (
        <FileTree
          entries={entries}
          abertos={abertos}
          selecionado={selecionado}
          onToggle={handleTogglePasta}
          onPick={handlePick}
          listaCortada={listaCortada}
          soModificados={soModificados}
          onToggleFiltro={handleToggleFiltro}
        />
      ) : null}

      {aba === 'busca' ? (
        <FileSearchBar q={qBusca} mode={modoBusca} resultados={resultados} buscaCortada={buscaCortada} onBusca={handleBusca} onPick={handlePick} />
      ) : null}

      {aba === 'arquivo' ? (
        editando && conteudo && conteudo.path === selecionado ? (
          <FileEditor
            path={selecionado!}
            initialText={conteudo.text}
            onSalvar={handleSalvar}
            onDescartar={() => setEditando(false)}
          />
        ) : selecionado ? (
          <FileViewer
            path={selecionado}
            conteudo={conteudo}
            diff={diff}
            loading={loading}
            erro={erro}
            escopo={escopo}
            onEscopo={(e) => void api.trocarEscopo(e)}
            onEditar={() => setEditando(true)}
            name={sessionName}
          />
        ) : (
          <View style={styles.center}>
            <Text style={[styles.muted, { color: theme.tokens.text.muted }]}>{m.arq_nada_mudou()}</Text>
          </View>
        )
      ) : null}

      {erro && aba !== 'arquivo' ? (
        <Text style={[styles.erro, { color: theme.tokens.status.error }]}>{erro}</Text>
      ) : null}
    </View>
  );
}

const styles = StyleSheet.create((theme) => ({
  root: {
    flex: 1,
  },
  tabs: {
    flexDirection: 'row',
    borderBottomWidth: 1,
  },
  tab: {
    flex: 1,
    paddingVertical: 10,
    alignItems: 'center',
    borderBottomWidth: 2,
    borderBottomColor: 'transparent',
  },
  tabTxt: {
    fontSize: 12,
    fontWeight: '600',
  },
  center: {
    flex: 1,
    alignItems: 'center',
    justifyContent: 'center',
  },
  muted: {
    fontSize: theme.base.text.xs,
  },
  erro: {
    fontSize: theme.base.text.xs,
    padding: theme.base.space[2],
  },
}));
