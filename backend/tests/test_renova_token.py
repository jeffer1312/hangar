"""Renovação do token OAuth das contas paradas — sem rede, sem tmux de verdade.

O contrato de I/O é o mesmo do `login_conta`: toda fala com o tmux passa pelas três privadas
`_criar_janela` / `_submeter` / `_matar`, trocadas aqui por dublê que só anota o que foi chamado.
A varredura de processos entra pelo `procinfo._pids_com_config_dir`, também trocada.

Os números do arquivo de credencial são MILISSEGUNDOS (é o que o Claude Code grava); os fixtures
abaixo escrevem nesse formato de propósito — um teste que usasse segundos passaria por acidente
justamente no bug que a conversão existe pra impedir.
"""
import json
import time
from pathlib import Path

import pytest

from app import contas, renova_token
from app.config import ConfigDirInfo


def _grava_credencial(d: Path, expira_em_s: float, refresh_em_s: float = 26 * 86400) -> None:
    agora = time.time()
    (d / ".credentials.json").write_text(json.dumps({"claudeAiOauth": {
        "accessToken": "sk-ant-oat-falso",
        "expiresAt": int((agora + expira_em_s) * 1000),
        "refreshTokenExpiresAt": int((agora + refresh_em_s) * 1000),
    }}), encoding="utf-8")


def _conta(tmp_path: Path, nome: str, *, expira_em_s: float = 8 * 3600,
           refresh_em_s: float = 26 * 86400, confiada: Path | None = None) -> Path:
    """Pasta de conta de verdade: com o marcador do app, pra o `contas.e_conta` passar sem dublê."""
    d = tmp_path / f".claude-{nome}"
    (d / "projects").mkdir(parents=True)
    (d / contas.MARCADOR).write_text("", encoding="utf-8")
    _grava_credencial(d, expira_em_s, refresh_em_s)
    projetos = {str(confiada): {"hasTrustDialogAccepted": True}} if confiada else {}
    (d / ".claude.json").write_text(json.dumps({"projects": projetos}), encoding="utf-8")
    return d


@pytest.fixture
def catalogo(monkeypatch):
    """Troca o `list_config_dirs` por uma lista fabricada de contas."""
    def definir(*dirs: Path):
        infos = [ConfigDirInfo(path=str(d), label=d.name, active=False) for d in dirs]
        monkeypatch.setattr(renova_token, "list_config_dirs", lambda: infos)
    return definir


@pytest.fixture
def sem_processos(monkeypatch):
    """Ninguém está usando config dir nenhum (varredura OK e vazia)."""
    monkeypatch.setattr(renova_token.procinfo, "_pids_com_config_dir", lambda alvo: ([], True))


class _Tmux:
    """Dublê do tmux: anota criações/digitações/mortes e nunca fala com o sistema."""

    def __init__(self, ao_submeter=None, criar_falha=False):
        self.criadas: list[tuple[str, str, str]] = []
        self.digitadas: list[tuple[str, str]] = []
        self.matadas: list[str] = []
        self.ao_submeter = ao_submeter
        self.criar_falha = criar_falha

    def instalar(self, monkeypatch):
        monkeypatch.setattr(renova_token, "_criar_janela", self.criar)
        monkeypatch.setattr(renova_token, "_submeter", self.submeter)
        monkeypatch.setattr(renova_token, "_matar", self.matar)
        # O poll real é de 1s; aqui o que importa é a lógica, não o relógio.
        monkeypatch.setattr(renova_token, "_POLL_S", 0.01)
        return self

    def criar(self, nome, cwd, dir_conta):
        if self.criar_falha:
            return None
        self.criadas.append((nome, cwd, dir_conta))
        return f"term-{nome}"

    def submeter(self, alvo, comando):
        self.digitadas.append((alvo, comando))
        if self.ao_submeter:
            self.ao_submeter()

    def matar(self, alvo):
        self.matadas.append(alvo)


# ----------------------------------------------------------------------- quem entra na lista


def test_token_longe_do_vencimento_fica_de_fora(tmp_path, catalogo):
    catalogo(_conta(tmp_path, "folgada", expira_em_s=7 * 3600))
    assert renova_token.contas_a_renovar() == []


def test_token_vencido_entra(tmp_path, catalogo):
    d = _conta(tmp_path, "vencida", expira_em_s=-3600)
    catalogo(d)
    assert renova_token.contas_a_renovar() == [d]


def test_token_dentro_da_margem_entra(tmp_path, catalogo):
    d = _conta(tmp_path, "quase", expira_em_s=1800)
    catalogo(d)
    assert renova_token.contas_a_renovar() == [d]


def test_expiresat_e_lido_em_milissegundos(tmp_path, catalogo):
    """Lendo o valor cru como SEGUNDOS, um token com 8h de vida pareceria vencido em 1970 e a
    rotina abriria uma sessão tmux por conta, toda rodada."""
    catalogo(_conta(tmp_path, "ms", expira_em_s=8 * 3600))
    assert renova_token.contas_a_renovar(margem_s=5400) == []


def test_conta_sem_credencial_fica_de_fora(tmp_path, catalogo):
    """Criada e nunca logada: não é token velho, é ausência de token — abrir `claude` ali cairia
    na tela de login, que nenhuma rotina de fundo resolve."""
    d = _conta(tmp_path, "virgem", expira_em_s=-3600)
    (d / ".credentials.json").unlink()
    catalogo(d)
    assert renova_token.contas_a_renovar() == []


def test_pasta_que_nao_e_conta_fica_de_fora(tmp_path, catalogo):
    """Mesmo filtro da aba Contas: `~/.claude-backup` com login legítimo continua no catálogo de
    custos, mas o app não abre sessão dentro da pasta de backup de ninguém."""
    d = _conta(tmp_path, "backup", expira_em_s=-3600)
    (d / contas.MARCADOR).unlink()
    catalogo(d)
    assert renova_token.contas_a_renovar() == []


# ---------------------------------------------------------------------------- pasta confiada


def test_pasta_confiada_devolve_a_primeira_existente(tmp_path):
    repo = tmp_path / "repo"
    repo.mkdir()
    d = _conta(tmp_path, "confia")
    (d / ".claude.json").write_text(json.dumps({"projects": {
        str(tmp_path / "sumiu"): {"hasTrustDialogAccepted": True},
        str(repo): {"hasTrustDialogAccepted": True},
    }}), encoding="utf-8")
    assert renova_token.pasta_confiada(d) == repo


def test_pasta_nao_confiada_nao_conta(tmp_path):
    repo = tmp_path / "repo"
    repo.mkdir()
    d = _conta(tmp_path, "naoconfia")
    (d / ".claude.json").write_text(json.dumps({"projects": {
        str(repo): {"hasTrustDialogAccepted": False},
    }}), encoding="utf-8")
    assert renova_token.pasta_confiada(d) is None


def test_claude_json_estragado_nao_levanta(tmp_path):
    d = _conta(tmp_path, "json-ruim")
    (d / ".claude.json").write_text("{ nao é json", encoding="utf-8")
    assert renova_token.pasta_confiada(d) is None


# ------------------------------------------------------------------------------- em uso


def test_varredura_que_falhou_conta_como_em_uso(tmp_path, monkeypatch):
    """"Não consegui olhar" não pode sair igual a "olhei e não tem ninguém": errar pro lado do não
    derruba a sessão de alguém."""
    d = _conta(tmp_path, "cega")
    monkeypatch.setattr(renova_token.procinfo, "_pids_com_config_dir", lambda alvo: ([], False))
    assert renova_token.esta_em_uso(d) is True


def test_proprio_backend_nao_conta_como_uso(tmp_path, monkeypatch):
    import os
    d = _conta(tmp_path, "eu")
    monkeypatch.setattr(renova_token.procinfo, "_pids_com_config_dir",
                        lambda alvo: ([os.getpid()], True))
    assert renova_token.esta_em_uso(d) is False


# ---------------------------------------------------------------------------------- renovar


def test_renovar_devolve_ok_quando_o_vencimento_avanca(tmp_path, monkeypatch, sem_processos):
    repo = tmp_path / "repo"
    repo.mkdir()
    d = _conta(tmp_path, "ok", expira_em_s=-3600, confiada=repo)
    # O `claude` subindo é o que reescreve a credencial: o dublê faz isso ao receber o comando.
    fake = _Tmux(ao_submeter=lambda: _grava_credencial(d, 8 * 3600)).instalar(monkeypatch)

    assert renova_token.renovar(d, espera_s=2) == (True, "renovado")
    assert fake.digitadas == [(f"term-{renova_token._nome_janela(d)}", "claude")]
    assert fake.criadas[0][1] == str(repo)          # cwd = a pasta confiada
    assert fake.criadas[0][2] == str(d)             # CLAUDE_CONFIG_DIR = a conta
    assert fake.matadas[-1] == f"term-{renova_token._nome_janela(d)}"


def test_renovar_mata_a_sessao_mesmo_no_timeout(tmp_path, monkeypatch, sem_processos):
    """Sessão órfã aqui vira pane fantasma — e um `claude` vivo segurando o config dir faria as
    rodadas seguintes acharem que a conta está "em uso"."""
    repo = tmp_path / "repo"
    repo.mkdir()
    d = _conta(tmp_path, "trava", expira_em_s=-3600, confiada=repo)
    fake = _Tmux().instalar(monkeypatch)      # ninguém reescreve a credencial

    assert renova_token.renovar(d, espera_s=0.05) == (False, "timeout")
    alvo = f"term-{renova_token._nome_janela(d)}"
    assert fake.matadas.count(alvo) >= 1


def test_renovar_mata_a_sessao_quando_o_envio_explode(tmp_path, monkeypatch, sem_processos):
    repo = tmp_path / "repo"
    repo.mkdir()
    d = _conta(tmp_path, "boom", expira_em_s=-3600, confiada=repo)

    def explode():
        raise RuntimeError("tmux recusou o send-keys")

    fake = _Tmux(ao_submeter=explode).instalar(monkeypatch)
    with pytest.raises(RuntimeError):
        renova_token.renovar(d, espera_s=1)
    assert f"term-{renova_token._nome_janela(d)}" in fake.matadas


def test_renovar_pula_conta_em_uso_sem_abrir_janela(tmp_path, monkeypatch):
    repo = tmp_path / "repo"
    repo.mkdir()
    d = _conta(tmp_path, "ocupada", expira_em_s=-3600, confiada=repo)
    monkeypatch.setattr(renova_token.procinfo, "_pids_com_config_dir", lambda alvo: ([4242], True))
    fake = _Tmux().instalar(monkeypatch)

    assert renova_token.renovar(d) == (False, "em-uso")
    assert fake.criadas == []


def test_renovar_pula_conta_sem_pasta_confiada(tmp_path, monkeypatch, sem_processos):
    """Abrir `claude` numa pasta não confiada trava na pergunta de confiança — a sessão ficaria
    parada até o timeout, sem renovar nada."""
    d = _conta(tmp_path, "sem-pasta", expira_em_s=-3600)
    fake = _Tmux().instalar(monkeypatch)

    assert renova_token.renovar(d) == (False, "sem-pasta-confiada")
    assert fake.criadas == []


def test_mtime_sem_vencimento_novo_nao_e_renovacao(tmp_path):
    """O mtime é só o portão barato: arquivo reescrito com o MESMO vencimento não é token novo."""
    assert renova_token._renovou((10.0, 100.0), (20.0, 100.0)) is False
    assert renova_token._renovou((10.0, 100.0), (20.0, 200.0)) is True
    assert renova_token._renovou((10.0, 100.0), (10.0, 200.0)) is False


# ----------------------------------------------------------------------------------- rodada


def test_rodada_relata_renovada_pulada_e_falha(tmp_path, monkeypatch, catalogo):
    repo = tmp_path / "repo"
    repo.mkdir()
    boa = _conta(tmp_path, "boa", expira_em_s=-3600, confiada=repo)
    ocupada = _conta(tmp_path, "ocupada", expira_em_s=-3600, confiada=repo)
    travada = _conta(tmp_path, "travada", expira_em_s=-3600, confiada=repo)
    catalogo(boa, ocupada, travada)
    monkeypatch.setattr(renova_token.procinfo, "_pids_com_config_dir",
                        lambda alvo: ([9], True) if Path(alvo) == ocupada else ([], True))
    _Tmux(ao_submeter=lambda: _grava_credencial(boa, 8 * 3600)).instalar(monkeypatch)
    # A rodada chama `renovar` com o padrão de 45s; a espera curta é a única diferença.
    original = renova_token.renovar
    monkeypatch.setattr(renova_token, "renovar", lambda d, espera_s=45: original(d, espera_s=0.05))

    rel = renova_token.rodada()
    assert rel["renovadas"] == [str(boa)]
    assert {"conta": str(ocupada), "motivo": "em-uso"} in rel["puladas"]
    assert {"conta": str(travada), "motivo": "timeout"} in rel["falhas"]


def test_rodada_pula_conta_com_refresh_vencido(tmp_path, monkeypatch, catalogo, sem_processos):
    """Renovar é impossível: o `claude` abriria pedindo login e a janela ficaria parada."""
    repo = tmp_path / "repo"
    repo.mkdir()
    d = _conta(tmp_path, "morta", expira_em_s=-3600, refresh_em_s=-86400, confiada=repo)
    catalogo(d)
    fake = _Tmux().instalar(monkeypatch)

    rel = renova_token.rodada()
    assert rel["puladas"] == [{"conta": str(d), "motivo": "refresh-vencido"}]
    assert fake.criadas == []


def test_rodada_nao_levanta_quando_um_leitor_explode(tmp_path, monkeypatch, catalogo,
                                                     sem_processos):
    """Uma conta estragada não pode derrubar a rotina nem impedir as outras de renovar."""
    repo = tmp_path / "repo"
    repo.mkdir()
    ruim = _conta(tmp_path, "ruim", expira_em_s=-3600, confiada=repo)
    boa = _conta(tmp_path, "boa", expira_em_s=-3600, confiada=repo)
    catalogo(ruim, boa)
    _Tmux(ao_submeter=lambda: _grava_credencial(boa, 8 * 3600)).instalar(monkeypatch)

    original = renova_token.pasta_confiada

    def pasta(d):
        if d == ruim:
            raise OSError("disco sumiu no meio da leitura")
        return original(d)

    monkeypatch.setattr(renova_token, "pasta_confiada", pasta)

    rel = renova_token.rodada()
    assert rel["renovadas"] == [str(boa)]
    assert rel["falhas"] and rel["falhas"][0]["conta"] == str(ruim)
    assert "OSError" in rel["falhas"][0]["motivo"]


def test_rodada_nao_levanta_quando_o_catalogo_explode(monkeypatch):
    def explode():
        raise OSError("home ilegível")

    monkeypatch.setattr(renova_token, "list_config_dirs", explode)
    assert renova_token.rodada() == {"renovadas": [], "puladas": [], "falhas": []}
