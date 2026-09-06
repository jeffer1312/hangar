import { DevSettings } from 'react-native';
import { Pagina } from '../../src/features/config/Pagina';
import { Linha } from '../../src/features/config/Linha';
import { Segmentado, type Opcao } from '../../src/features/config/Segmentado';
import { useAparencia, type Idioma, type Tema } from '../../src/stores/aparencia';
import { toast } from '../../src/ui/Toast';
import type { GroupBy, PensamentoTools } from '@hangar/core';
import * as m from '../../src/paraglide/messages';

export default function Geral() {
  const idioma = useAparencia((s) => s.idioma);
  const tema = useAparencia((s) => s.tema);
  const pensamento = useAparencia((s) => s.pensamentoTools);
  const agrupar = useAparencia((s) => s.agrupar);

  const IDIOMAS: ReadonlyArray<Opcao<Idioma>> = [
    { v: 'system', label: m.config_tema_auto(), aria: m.config_idioma_sistema() },
    { v: 'pt', label: m.config_idioma_pt() },
    { v: 'en', label: m.config_idioma_en() },
  ];
  const TEMAS: ReadonlyArray<Opcao<Tema>> = [
    { v: 'system', label: m.config_tema_auto(), aria: m.config_idioma_sistema() },
    { v: 'light', label: m.config_tema_claro() },
    { v: 'dark', label: m.config_tema_escuro() },
  ];
  const PENSAMENTO: ReadonlyArray<Opcao<PensamentoTools>> = [
    { v: 'nada', label: m.config_aparencia_pensamento_nada(), aria: m.config_aparencia_pensamento_nada_aria() },
    { v: 'busca', label: m.config_aparencia_pensamento_busca(), aria: m.config_aparencia_pensamento_busca_aria() },
    { v: 'tudo', label: m.config_aparencia_pensamento_tudo(), aria: m.config_aparencia_pensamento_tudo_aria() },
  ];
  const AGRUPAR: ReadonlyArray<Opcao<GroupBy>> = [
    { v: 'server', label: m.lista_agrupar_servidor() },
    { v: 'project', label: m.lista_agrupar_projeto() },
    { v: 'none', label: m.lista_agrupar_nenhum() },
  ];

  // Trocar o idioma não troca as mensagens já carregadas (o paraglide as compila em funções): em
  // dev o recarregador resolve na hora, em produção só na próxima abertura — e o aviso diz qual.
  const trocarIdioma = (v: Idioma) => {
    useAparencia.getState().setIdioma(v);
    toast.ok(__DEV__ ? m.config_idioma_nota_reload() : m.config_idioma_nota_proxima());
    if (__DEV__) DevSettings.reload();
  };

  return (
    <Pagina>
      <Linha titulo={m.config_idioma_rotulo()} descricao={m.config_idioma_nota_proxima()}>
        <Segmentado opcoes={IDIOMAS} valor={idioma} onChange={trocarIdioma} rotulo={m.config_idioma_rotulo()} />
      </Linha>
      <Linha titulo={m.config_tema_curto()} descricao={m.config_aparencia_tema_desc()}>
        <Segmentado opcoes={TEMAS} valor={tema} onChange={(v) => useAparencia.getState().setTema(v)} rotulo={m.config_tema_curto()} />
      </Linha>
      <Linha
        titulo={m.config_aparencia_pensamento_tools()}
        descricao={m.config_aparencia_pensamento_tools_desc()}
      >
        <Segmentado
          opcoes={PENSAMENTO}
          valor={pensamento}
          onChange={(v) => useAparencia.getState().setPensamentoTools(v)}
          rotulo={m.config_aparencia_pensamento_tools()}
        />
      </Linha>
      <Linha titulo={m.lista_agrupar()}>
        <Segmentado opcoes={AGRUPAR} valor={agrupar} onChange={(v) => useAparencia.getState().setAgrupar(v)} rotulo={m.lista_agrupar()} />
      </Linha>
    </Pagina>
  );
}
