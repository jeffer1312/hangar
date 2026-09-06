import { useEffect, useState } from 'react';
import { Linking, Text } from 'react-native';
import { StyleSheet } from 'react-native-unistyles';
import Constants from 'expo-constants';
import { getConfig } from '@hangar/core';
import { Pagina } from '../../src/features/config/Pagina';
import { Linha } from '../../src/features/config/Linha';
import { toast } from '../../src/ui/Toast';
import { useServers } from '../../src/stores/servers';
import * as m from '../../src/paraglide/messages';

const REPO = 'https://github.com/jeffer1312/hangar';
// O backend expõe a versão dele em `somente_leitura`, mas o nome da chave já mudou de mão:
// mostrar a primeira que vier evita uma tela muda a cada renomeação lá.
const CHAVES_VERSAO = ['versao', 'version', 'commit'] as const;

export default function Sobre() {
  const ativo = useServers((s) => s.servers.find((x) => x.id === s.activeId) ?? null);
  const [servidor, setServidor] = useState<string | null>(null);
  const [falhou, setFalhou] = useState(false);

  useEffect(() => {
    let vivo = true;
    // Zera o aviso a cada tentativa: sem isto, uma falha deixava o texto de erro na tela pra sempre.
    setFalhou(false);
    getConfig()
      .then((c) => {
        if (!vivo) return;
        const chave = CHAVES_VERSAO.find((k) => c.somente_leitura[k] !== undefined);
        setServidor(chave ? String(c.somente_leitura[chave]) : null);
      })
      .catch(() => { if (vivo) setFalhou(true); });
    return () => { vivo = false; };
  }, [ativo?.baseUrl]);

  const abrirRepo = () => {
    Linking.openURL(REPO).catch((e: unknown) =>
      toast.erro(e instanceof Error ? e.message : m.erro_desconhecido()),
    );
  };

  const versao = Constants.expoConfig?.version ?? null;
  const runtime = Constants.expoConfig?.runtimeVersion;

  return (
    <Pagina>
      <Text style={styles.desc}>{m.config_sobre_desc()}</Text>
      {versao ? (
        <Linha
          titulo={m.atualizar_versao()}
          descricao={typeof runtime === 'string' ? `${versao} · ${runtime}` : versao}
        />
      ) : null}
      <Linha
        titulo={m.config_sobre_servidor()}
        descricao={ativo?.baseUrl ?? m.maquinas_vazio()}
        direita={
          falhou ? <Text style={styles.erro}>{m.atualizar_procurar_falhou()}</Text> : null
        }
      />
      {servidor ? <Linha titulo={m.atualizar_versao_servidor()} descricao={servidor} /> : null}
      <Linha icon="ExternalLink" titulo={REPO.replace('https://', '')} onPress={abrirRepo} />
    </Pagina>
  );
}

const styles = StyleSheet.create((theme) => ({
  desc: { fontSize: theme.base.text.sm, color: theme.tokens.text.secondary, paddingHorizontal: theme.base.space[1] },
  erro: { fontSize: theme.base.text.xxs, color: theme.tokens.status.error },
}));
