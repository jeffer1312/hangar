"""A raiz do agente omp tem UMA resposta: quem lê (sessões, saúde, login) e quem escreve
(plugin sync, contexto) enxergam o mesmo perfil."""
from pathlib import Path

import pytest

from app import oauth_codex, omp_dirs


@pytest.fixture
def home(tmp_path, monkeypatch, hangar_server_bin):
    monkeypatch.setattr(Path, "home", classmethod(lambda cls: tmp_path))
    for key in ("OMP_PROFILE", "PI_PROFILE", "PI_CONFIG_DIR", "PI_CODING_AGENT_DIR", "XDG_DATA_HOME"):
        monkeypatch.delenv(key, raising=False)
    return tmp_path


def test_sem_perfil_todos_apontam_para_omp_agent(home):
    raiz = home / ".omp" / "agent"
    assert omp_dirs.agent_dir() == raiz
    assert oauth_codex._omp_db(None) == raiz / "agent.db"


def test_com_perfil_leitores_e_escritores_concordam(home, monkeypatch):
    monkeypatch.setenv("OMP_PROFILE", "trabalho")
    raiz = home / ".omp" / "profiles" / "trabalho" / "agent"
    escritor = omp_dirs.agent_dir(home=home, env={"OMP_PROFILE": "trabalho"}, estrito=True)
    assert escritor == raiz
    assert oauth_codex._omp_db(None) == raiz / "agent.db"


def test_perfil_invalido_derruba_quem_escreve(home, monkeypatch):
    # O login do ChatGPT grava credencial no agent.db: cair calado na raiz sem perfil gravaria
    # no lugar errado com o relatório dizendo "ok".
    monkeypatch.setenv("OMP_PROFILE", "Nome Inválido")
    with pytest.raises(ValueError, match="perfil do omp inválido"):
        oauth_codex._omp_db(None)


def test_home_explicita_e_raiz_fixa(home, monkeypatch):
    monkeypatch.setenv("OMP_PROFILE", "trabalho")
    outra = home / "outra"
    assert oauth_codex._omp_db(outra) == outra / ".omp" / "agent" / "agent.db"
