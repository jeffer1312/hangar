import { useEffect, useState } from 'react';
import { Linking, Text } from 'react-native';
import { StyleSheet } from 'react-native-unistyles';
import Constants from 'expo-constants';
import { getConfig } from '@hangar/core';
import { Pagina } from '../../src/features/config/Pagina';
import { PageHeader } from '../../src/features/config/PageHeader';
import { SectionCard } from '../../src/features/config/SectionCard';
import { SettingsRow } from '../../src/features/config/SettingsRow';
import { LastErrorSection } from '../../src/features/config/LastErrorSection';
import { toast } from '../../src/ui/Toast';
import { useServers } from '../../src/stores/servers';
import * as m from '../../src/paraglide/messages';

const REPO = 'https://github.com/jeffer1312/hangar';
const PRIVACY_URL = 'https://hangar.dev.br/privacy';
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

  const abrirLink = (url: string) => {
    Linking.openURL(url).catch((e: unknown) =>
      toast.erro(e instanceof Error ? e.message : m.erro_desconhecido()),
    );
  };

  const versao = Constants.expoConfig?.version ?? null;
  const runtime = Constants.expoConfig?.runtimeVersion;

  return (
    <Pagina>
      <PageHeader
        title={m.config_modal_sobre()}
        subtitle={m.config_sobre_desc()}
        actions={[{ icon: 'ExternalLink', label: REPO.replace('https://', ''), onPress: () => abrirLink(REPO) }]}
      />
      {versao ? (
        <SectionCard icon="Smartphone" title={m.native_settings_about_app_group()}>
          <SettingsRow
            title={m.atualizar_versao()}
            description={typeof runtime === 'string' ? `${versao} · ${runtime}` : versao}
          />
          <SettingsRow title={m.mobile_privacy_policy()} onPress={() => abrirLink(PRIVACY_URL)} />
        </SectionCard>
      ) : null}
      <LastErrorSection />
      <SectionCard icon="Server" title={m.native_settings_about_server_group()}>
        <SettingsRow
          title={m.config_sobre_servidor()}
          description={ativo?.baseUrl ?? m.maquinas_vazio()}
          right={falhou ? <Text style={styles.erro}>{m.atualizar_procurar_falhou()}</Text> : null}
        />
        {servidor ? <SettingsRow title={m.atualizar_versao_servidor()} description={servidor} /> : null}
      </SectionCard>
    </Pagina>
  );
}

const styles = StyleSheet.create((theme) => ({
  erro: { fontSize: theme.base.text.xxs, color: theme.tokens.status.error },
}));
