"""Mensagens da integração Codex que chegam à tela: código + parâmetros, com o texto em pt junto.

O contrato do app é o backend mandar código e o front traduzir (`harness_codex_m_<codigo>` nos
`messages/*.json`). O texto continua aqui porque log, lançador da TUI e testes o leem — e uma
`Mensagem` É uma `str`, então quem já comparava texto não muda."""
from __future__ import annotations

CATALOGO: dict[str, str] = {
    "etapa_reinicio": "Aguardando reconciliação após reinício",
    "etapa_aguardando": "Aguardando integração",
    "etapa_inventariando": "Inventariando configuração",
    "etapa_sem_claude": "Claude Code sem configuração nesta máquina",
    "etapa_sem_codex": "Codex CLI não encontrado",
    "etapa_instrucoes": "Preparando instruções e hooks",
    "etapa_importando": "Importando pelo Codex",
    "etapa_fragmentos": "Reconciliando hooks, comandos, agentes e MCPs",
    "etapa_skills": "Atualizando ponte de skills",
    "etapa_interrompida": "Reconciliação interrompida; será retomada",
    "etapa_concluido": "Concluído",
    "etapa_pendencias": "Confira os itens pendentes",
    "etapa_marketplace": "Atualizando marketplace {marketplace}",
    "etapa_plugin": "Instalando ou atualizando {id}",
    "erro_registro_ilegivel": "Registro da integração ilegível",
    "erro_falha": "Falha na integração ({tipo}); configuração anterior preservada nas etapas não concluídas",
    "erro_falha_inesperada": "Falha inesperada na integração ({tipo}); configuração anterior preservada nas etapas não concluídas",
    "erro_marketplace_origem": "Origem do marketplace {marketplace} não confirmada; instalação existente preservada",
    "erro_plugins_incompletos": "Importação nativa de plugins incompleta; confira os plugins pendentes",
    "erro_marketplace_atualizar": "Não foi possível atualizar o marketplace {marketplace}",
    "erro_marketplace_pendente": "Atualização do marketplace {marketplace} permanece pendente; nova tentativa agendada",
    "erro_plugin": "Não foi possível reconciliar o plugin {id}",
    "aviso_hooks_alterados": "Hooks alterados: confira a aprovação dos hooks no Codex antes de usá-los.",
    "aviso_confianca_indisponivel": "Esta versão do Codex não informou a confiança dos hooks.",
    "aviso_hooks_sem_arquivo": "Hooks sem arquivo no Codex e sem equivalente em ~/.claude/hooks: {arquivos}",
    "aviso_historico_indisponivel": "Histórico nativo indisponível; colisões existentes serão preservadas.",
    "aviso_ignorados": "Não reconhecidos pelo Codex e ignorados em {pasta}/: {arquivos}",
    "aviso_artefato_sem_proveniencia": "Artefato sem proveniência preservado: {path}",
    "aviso_artefato_falha": "Artefato {path} preservado após falha: {erro}",
    "aviso_artefato_obsoleto_alterado": "Artefato obsoleto com alteração local preservado: {path}",
    "aviso_artefato_obsoleto_falha": "Artefato obsoleto {path} preservado após falha: {erro}",
    "aviso_config_sem_proveniencia": "Entrada de configuração sem proveniência preservada: {nome}",
    "aviso_config_campos_particulares": "Campos particulares preservados na entrada removida da fonte: {nome}",
    "aviso_plugin_skills": "Skills do plugin {id} não puderam ser confirmadas: {erro}",
    "aviso_arquivo_exclusivo": "Arquivo exclusivo preservado: {path}",
    "aviso_arquivo_falha": "Arquivo {path} preservado após falha: {erro}",
    "aviso_arquivo_removido_alterado": "Arquivo removido na fonte, mas alterado localmente, preservado: {path}",
    "aviso_ponte_alteracao_local": "Alteração local preservada ao retirar ponte: {path}",
    "aviso_ponte_conteudo_pessoal": "Conteúdo pessoal preservado na ponte desnecessária: {destino}",
    "aviso_skill_nativa_sem_proveniencia": "Skill nativa sem proveniência preservada: {destino}",
    "aviso_skill_duplicata_plugin": "Skill {nome} existe no plugin nativo e em ~/.agents/skills; a cópia pessoal foi preservada",
    "aviso_skill_agents_difere": "Skill {nome} em ~/.agents/skills difere da fonte do Claude; o Codex usa a de ~/.agents/skills",
    "aviso_link_pessoal": "Link pessoal preservado: {destino}",
    "aviso_skill_pessoal": "Skill pessoal preservada: {destino}",
    "aviso_skill_falha": "Skill {nome} preservada após falha: {erro}",
    "aviso_skill_nome_invalido": "Nome inválido no manifesto de skills preservado: {nome}",
    "aviso_copia_obsoleta_falha": "Cópia obsoleta {nome} preservada após falha: {erro}",
    "aviso_skills_sem_fonte": "Nenhuma fonte de skills disponível; pontes anteriores preservadas.",
}


class Mensagem(str):
    codigo: str
    params: dict[str, str]

    def __new__(cls, codigo: str, **params):
        texto = CATALOGO[codigo].format(**params)
        self = super().__new__(cls, texto)
        self.codigo = codigo
        self.params = {k: str(v) for k, v in params.items()}
        return self

    # `copy.deepcopy`/pickle reconstroem uma str pelo valor; aqui o valor não é o código.
    def __reduce__(self):
        return (_reconstruir, (self.codigo, self.params))

    def __deepcopy__(self, memo):
        return Mensagem(self.codigo, **self.params)

    def __copy__(self):
        return Mensagem(self.codigo, **self.params)


def _reconstruir(codigo: str, params: dict) -> "Mensagem":
    return Mensagem(codigo, **params)


def msg(codigo: str, **params) -> Mensagem:
    return Mensagem(codigo, **params)


def serializar(valor) -> dict:
    """Forma que vai pro JSON (e pro `estado.json`): dict já serializado passa; texto solto vai sem código."""
    if isinstance(valor, dict):
        return valor
    if isinstance(valor, Mensagem):
        return {"codigo": valor.codigo, "params": valor.params, "texto": str(valor)}
    return {"codigo": None, "params": {}, "texto": "" if valor is None else str(valor)}
