// Traducao dos erros de API que o backend manda no formato {code, params, msg}
// (backend/app/mensagens.py). O `code` e o contrato: e por ele que o front acha a mensagem
// no idioma do app. `params` sao os valores que entram na frase; `msg` e o texto em portugues
// que o backend mandou junto, e so a rede quando o code e desconhecido.
//
// Mapa EXPLICITO, nunca `m['erro_' + code]`: o acesso dinamico anula o tipo gerado pelo
// Paraglide, que e metade da trava deste mecanismo. Code fora do mapa devolve `undefined`
// e o errorDetail cai no `msg` — backend mais novo que o front mostra texto legivel em vez
// de um codigo cru na tela.
import * as m from '../paraglide/messages';

type Parametros = Record<string, unknown>;

const ERROS: Record<string, (params: Parametros) => string> = {
  // /api/claude-configs — apagar conta recusado por alguma condicao da maquina
  erro_config_dirs_fixo: () => m.erro_config_dirs_fixo(),
  erro_conta_ativa_backend: () => m.erro_conta_ativa_backend(),
  erro_conta_lista_fixa: () => m.erro_conta_lista_fixa(),
  erro_config_dir_sessao: (p) => m.erro_config_dir_sessao({ nome: String(p.nome) }),
  erro_sessao_usa_conta: (p) => m.erro_sessao_usa_conta({ nome: String(p.nome) }),
  erro_varredura_processos: () => m.erro_varredura_processos(),
  erro_processos_usam_conta: (p) => m.erro_processos_usam_conta({ pids: String(p.pids) }),

  // /api/desktop — a paleta e o papel de parede sao resposta de negocio, nao erro
  erro_sem_paleta: () => m.erro_sem_paleta(),
  erro_sem_papel_de_parede: () => m.erro_sem_papel_de_parede(),

  // /api/broadcast — slash-command nao viaja no fan-out
  erro_broadcast_slash: () => m.erro_broadcast_slash(),

  // /api/config e /api/engines — corpo que nao e objeto
  erro_corpo_deve_ser_objeto: () => m.erro_corpo_deve_ser_objeto(),

  // /api/engines — motor nao existe, nome misturado com dados, ou faltando
  erro_motor_nao_encontrado: () => m.erro_motor_nao_encontrado(),
  erro_motor_nome_com_dados: () => m.erro_motor_nome_com_dados(),
  erro_motor_nome_ou_dados: () => m.erro_motor_nome_ou_dados(),

  // /api/model-options — catalogo do Pi falhou ou provider desconhecido
  erro_pi_list_models: (p) => m.erro_pi_list_models({ erro: String(p.erro) }),
  erro_provider_invalido: () => m.erro_provider_invalido(),

  // /api/archive — arquivo inexistente, path invalido, motor errado no resume
  erro_path_invalido: () => m.erro_path_invalido(),
  erro_projeto_nao_encontrado: () => m.erro_projeto_nao_encontrado(),
  erro_transcript_nao_encontrado: () => m.erro_transcript_nao_encontrado(),
  erro_nao_encontrado: () => m.erro_nao_encontrado(),
  erro_imagem_nao_encontrada: () => m.erro_imagem_nao_encontrada(),
  erro_cwd_ausente: () => m.erro_cwd_ausente(),
  erro_motor_invalido: () => m.erro_motor_invalido(),

  // /api/ask-history — kill-switch desligado
  erro_automacoes_desligadas: () => m.erro_automacoes_desligadas(),

  // /api/tts — texto vazio apos limpar, teto, limite, audio sumido do cache
  erro_tts_sem_texto: () => m.erro_tts_sem_texto(),
  erro_tts_teto: (p) => m.erro_tts_teto({ n: String(p.n), teto: String(p.teto) }),
  erro_tts_limite: (p) => m.erro_tts_limite({ n: String(p.n), limite: String(p.limite) }),
  erro_tts_audio_invalido: () => m.erro_tts_audio_invalido(),
  erro_tts_sem_cache: () => m.erro_tts_sem_cache(),

  // /api/sync — hub zero-knowledge (login, cadastro, rate limit)
  erro_nao_autorizado: () => m.erro_nao_autorizado(),
  erro_bootstrap_invalido: () => m.erro_bootstrap_invalido(),
  erro_ja_registrado: () => m.erro_ja_registrado(),
  erro_muitas_tentativas: () => m.erro_muitas_tentativas(),

  // /api/deploy — webhook do GitHub (o front nunca chama; a traducao existe pro caso de um dia)
  erro_assinatura_invalida: () => m.erro_assinatura_invalida(),
  erro_payload_invalido: () => m.erro_payload_invalido(),
  erro_deploy_falhou: () => m.erro_deploy_falhou(),

  // require_auth / require_loopback — 429 do backoff, 403 do loopback
  erro_so_loopback: () => m.erro_so_loopback(),
};

export function mensagemDeErro(code: string, params: Parametros = {}): string | undefined {
  // Propriedade PROPRIA, nunca a leitura crua: nome herdado do prototipo (toString, constructor,
  // hasOwnProperty, __proto__) existe em todo objeto e chamaria a funcao errada — medido:
  // `ERROS['toString']` devolve a funcao herdada e a chamada retorna '[object Undefined]'.
  if (!Object.prototype.hasOwnProperty.call(ERROS, code)) return undefined;
  return ERROS[code](params);
}
