import { useEffect, useState } from 'react';
import { ActivityIndicator, Pressable, Text, View, ScrollView } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { WebView } from 'react-native-webview';
import type { FileContent, PathDiff } from '@hangar/core';
import { fileUrlNative, fileAuthHeader, mensagemDeErro } from '@hangar/core';
import * as m from '../../paraglide/messages';
import { DiffView } from '../../vendor/happy/components/diff/DiffView';

interface Props {
  path: string;
  conteudo: FileContent | null;
  diff: PathDiff | null;
  loading: boolean;
  erro?: string | null;
  escopo: 'branch' | 'nao_commitado';
  onEscopo: (e: 'branch' | 'nao_commitado') => void;
  onEditar: () => void;
  name: string; // sessão name para fileUrl
}

function isHtmlPdf(path: string) {
  const low = path.toLowerCase();
  return low.endsWith('.html') || low.endsWith('.htm') || low.endsWith('.pdf');
}

export function FileViewer({ path, conteudo, diff, loading, erro, escopo, onEscopo, onEditar, name }: Props) {
  const { theme } = useUnistyles();
  const [verArquivo, setVerArquivo] = useState(false);
  const [webErro, setWebErro] = useState<string | null>(null);
  const [webCarregando, setWebCarregando] = useState(true);

  const doArquivo = conteudo && conteudo.path === path ? conteudo : null;
  const diffDoArquivo = diff && diff.path === path ? diff : null;
  const temDiff = !!diffDoArquivo && diffDoArquivo.diff.trim() !== '';
  const podeEditar = !!doArquivo && doArquivo.digest !== null;

  // html/pdf via WebView — modo nativo: token só no header GET, nunca na URL (BLOQUEADOR 1)
  // Guarda de carregamento/erro do WebView (BLOQUEADOR 4): nunca área vazia
  useEffect(() => {
    setWebErro(null);
    setWebCarregando(true);
  }, [path, name]);

  if (isHtmlPdf(path) && doArquivo) {
    const uri = fileUrlNative(name, path);
    const headers = fileAuthHeader();
    const semToken = Object.keys(headers).length === 0;
    if (semToken) {
      return (
        <View style={styles.root}>
          <Text style={[styles.erro, { color: theme.tokens.status.error }]} accessibilityRole="alert">
            {m.sessao_expirada()}
          </Text>
        </View>
      );
    }
    return (
      <View style={styles.root}>
        <View style={[styles.bar, { borderBottomColor: theme.tokens.border.subtle }]}>
          <Text style={[styles.path, { color: theme.tokens.text.primary }]} numberOfLines={1}>
            {path}
          </Text>
        </View>
        {webCarregando && !webErro ? (
          <View style={styles.webLoading}>
            <ActivityIndicator color={theme.tokens.text.muted} />
            <Text style={[styles.aviso, { color: theme.tokens.text.muted }]}>{m.comum_carregando()}</Text>
          </View>
        ) : null}
        {webErro ? (
          <Text style={[styles.erro, { color: theme.tokens.status.error }]} accessibilityRole="alert">
            {webErro}
          </Text>
        ) : null}
        <WebView
          source={{ uri, headers }}
          style={styles.webview}
          onLoadStart={() => {
            setWebCarregando(true);
          }}
          onLoad={() => setWebCarregando(false)}
          onError={(e) => {
            setWebCarregando(false);
            setWebErro(e.nativeEvent.description || m.arquivo_carregar_erro());
          }}
          onHttpError={(e) => {
            setWebCarregando(false);
            const descricao = e.nativeEvent.description;
            setWebErro(descricao ? `HTTP ${e.nativeEvent.statusCode}: ${descricao}` : m.arquivo_carregar_erro());
          }}
        />
      </View>
    );
  }

  if (erro) {
    return (
      <View style={styles.root}>
        <Text style={[styles.erro, { color: theme.tokens.status.error }]} accessibilityRole="alert">
          {erro}
        </Text>
      </View>
    );
  }

  if (loading) {
    return (
      <View style={styles.root}>
        <Text style={[styles.aviso, { color: theme.tokens.text.muted }]}>{m.git_diff_carregando()}</Text>
      </View>
    );
  }

  // sem arquivo ainda
  if (!doArquivo && !temDiff) {
    return (
      <View style={styles.root}>
        <Text style={[styles.aviso, { color: theme.tokens.text.muted }]}>{m.arq_sem_mudanca()}</Text>
      </View>
    );
  }

  const linhas = doArquivo ? (doArquivo.text === '' ? [] : doArquivo.text.replace(/\n$/, '').split('\n')) : [];
  const caiu = diffDoArquivo !== null && diffDoArquivo.escopo_usado !== diffDoArquivo.escopo_pedido;
  const motivoVisivel =
    caiu && diffDoArquivo?.motivo ? (mensagemDeErro(diffDoArquivo.motivo) ?? diffDoArquivo.motivo) : null;

  const diffTruncated = diffDoArquivo?.truncated ?? false;
  const arquivoTruncated = doArquivo?.truncated ?? false;

  return (
    <View style={styles.root}>
      {/* header */}
      <View style={[styles.bar, { borderBottomColor: theme.tokens.border.subtle }]}>
        <Text style={[styles.path, { color: theme.tokens.text.primary }]} numberOfLines={1}>
          {path}
        </Text>
        {podeEditar ? (
          <Pressable onPress={onEditar} style={styles.editBtn} accessibilityRole="button" accessibilityLabel={m.arq_editar()}>
            <Text style={[styles.editTxt, { color: theme.tokens.accent.base }]}>{m.arq_editar()}</Text>
          </Pressable>
        ) : null}
      </View>

      {/* toggle arquivo / alterações */}
      {temDiff && diffDoArquivo ? (
        <View style={styles.segRow}>
          <View style={[styles.seg, { borderColor: theme.tokens.border.subtle }]}>
            <Pressable
              onPress={() => setVerArquivo(false)}
              style={[styles.segBtn, !verArquivo ? { backgroundColor: theme.tokens.accent.dim } : null]}
            >
              <Text style={{ color: !verArquivo ? theme.tokens.accent.base : theme.tokens.text.secondary, fontSize: 12 }}>{m.arq_ver_alteracoes()}</Text>
            </Pressable>
            <Pressable
              onPress={() => setVerArquivo(true)}
              style={[styles.segBtn, verArquivo ? { backgroundColor: theme.tokens.accent.dim } : null]}
            >
              <Text style={{ color: verArquivo ? theme.tokens.accent.base : theme.tokens.text.secondary, fontSize: 12 }}>{m.arq_ver_arquivo()}</Text>
            </Pressable>
          </View>
          <Pressable
            onPress={() => onEscopo(escopo === 'branch' ? 'nao_commitado' : 'branch')}
            disabled={!!caiu}
            style={styles.escopoBtn}
          >
            <Text style={[styles.escopoTxt, { color: theme.tokens.text.muted }]}>
              {diffDoArquivo.base ? m.arq_escopo_desde({ base: diffDoArquivo.base.slice(0, 7) }) : escopo === 'branch' ? m.arq_escopo_branch() : m.arq_escopo_nao_commitado()}
            </Text>
          </Pressable>
          {motivoVisivel ? <Text style={[styles.motivo, { color: theme.tokens.status.warning }]}>{motivoVisivel}</Text> : null}
        </View>
      ) : (
        <View style={styles.metaRow}>
          <Pressable
            onPress={() => onEscopo(escopo === 'branch' ? 'nao_commitado' : 'branch')}
            disabled={!!caiu}
            style={styles.escopoBtn}
          >
            <Text style={[styles.escopoTxt, { color: theme.tokens.text.muted }]}>
              {diffDoArquivo?.base ? m.arq_escopo_desde({ base: diffDoArquivo.base.slice(0, 7) }) : escopo === 'branch' ? m.arq_escopo_branch() : m.arq_escopo_nao_commitado()}
            </Text>
          </Pressable>
        </View>
      )}

      {arquivoTruncated ? <Text style={[styles.aviso, { color: theme.tokens.status.warning }]}>{m.arq_arquivo_cortado()}</Text> : null}
      {diffTruncated && temDiff && !verArquivo ? <Text style={[styles.aviso, { color: theme.tokens.status.warning }]}>{m.arq_diff_cortado()}</Text> : null}

      {/* conteúdo */}
      {temDiff && !verArquivo && diffDoArquivo ? (
        <ScrollView style={styles.scroll}>
          <DiffView oldText={diffDoArquivo.original ?? ''} newText={doArquivo?.text ?? ''} showLineNumbers={true} />
          {/* fallback: se DiffView não renderiza, mostra raw diff */}
          {diffDoArquivo.original === null && diffDoArquivo.diff ? (
            <Text style={[styles.mono, { color: theme.tokens.text.secondary }]} selectable>
              {diffDoArquivo.diff}
            </Text>
          ) : null}
        </ScrollView>
      ) : doArquivo ? (
        <ScrollView style={styles.scroll}>
          <View style={styles.codeView}>
            {linhas.map((linha, i) => (
              <View key={i} style={styles.lineRow}>
                <Text style={[styles.lineNo, { color: theme.tokens.text.muted }]}>{i + 1}</Text>
                <Text style={[styles.lineTxt, { color: theme.tokens.text.primary }]} selectable>
                  {linha || ' '}
                </Text>
              </View>
            ))}
          </View>
        </ScrollView>
      ) : null}
    </View>
  );
}

const styles = StyleSheet.create((theme) => ({
  root: {
    flex: 1,
    backgroundColor: theme.tokens.bg.base,
  },
  bar: {
    flexDirection: 'row',
    alignItems: 'center',
    paddingHorizontal: theme.base.space[3],
    paddingVertical: theme.base.space[2],
    borderBottomWidth: 1,
    gap: theme.base.space[2],
  },
  path: {
    flex: 1,
    fontSize: 12,
    fontWeight: '500',
  },
  editBtn: {
    paddingHorizontal: 10,
    paddingVertical: 6,
    borderRadius: 8,
    backgroundColor: theme.tokens.accent.dim,
  },
  editTxt: {
    fontSize: 12,
    fontWeight: '600',
  },
  segRow: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 8,
    paddingHorizontal: theme.base.space[3],
    paddingVertical: 6,
  },
  metaRow: {
    flexDirection: 'row',
    paddingHorizontal: theme.base.space[3],
    paddingVertical: 4,
  },
  seg: {
    flexDirection: 'row',
    borderWidth: 1,
    borderRadius: 8,
    overflow: 'hidden',
  },
  segBtn: {
    paddingHorizontal: 10,
    paddingVertical: 6,
  },
  escopoBtn: {
    paddingHorizontal: 6,
  },
  escopoTxt: {
    fontSize: 11,
  },
  motivo: {
    fontSize: 11,
    flex: 1,
  },
  aviso: {
    fontSize: theme.base.text.xs,
    paddingHorizontal: theme.base.space[3],
    paddingVertical: 4,
  },
  erro: {
    fontSize: theme.base.text.xs,
    padding: theme.base.space[3],
  },
  scroll: {
    flex: 1,
  },
  webview: {
    flex: 1,
  },
  webLoading: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 8,
    padding: theme.base.space[3],
  },
  codeView: {
    padding: theme.base.space[2],
  },
  lineRow: {
    flexDirection: 'row',
    gap: 8,
  },
  lineNo: {
    width: 32,
    textAlign: 'right',
    fontSize: 11,
    fontFamily: theme.base.fontMono,
  },
  lineTxt: {
    flex: 1,
    fontSize: 12,
    fontFamily: theme.base.fontMono,
  },
  mono: {
    fontSize: 11,
    fontFamily: theme.base.fontMono,
    padding: theme.base.space[3],
  },
}));
