"""Contas Claude do lado Python: nomes, listagem e a chamada ao `hangar-server conta`.

A reconciliação e a criação são do Rust (`crates/hangar-server/src/accounts/claude_config.rs`); o
comportamento delas é provado lá (`tests/it/accounts_reconcile.rs`) contra fixtures gravadas do
Python de origem. Aqui ficam o que continua no Python e o contrato da chamada e do lançador.
"""
import json
import subprocess
import sys
from pathlib import Path

import pytest

from app import contas

SCRIPT = Path(__file__).resolve().parents[2] / "scripts" / "hangar-conta"


@pytest.fixture
def casa(tmp_path, monkeypatch):
    monkeypatch.setenv("HOME", str(tmp_path))
    (tmp_path / ".claude" / "skills").mkdir(parents=True)
    (tmp_path / ".claude" / "settings.json").write_text('{"theme":"dark"}', encoding="utf-8")
    (tmp_path / ".claude.json").write_text(json.dumps({"oauthAccount": {"x": 1}, "mcpServers": {}}), encoding="utf-8")
    return tmp_path


def _carimbada(casa: Path, nome: str) -> Path:
    conta = casa / f".claude-{nome}"
    conta.mkdir()
    (conta / contas.MARCADOR).write_text("", encoding="utf-8")
    return conta


def _diario_do_rust(casa: Path) -> list[Path]:
    return list((casa / ".hangar" / "logs" / "diario" / "hangar-conta").glob("uso-*.jsonl"))


def test_nome_fora_da_regra_e_recusado_antes_do_disco(casa):
    """fullmatch e não match+$: `conta2\\n` passava e a pasta nascia com quebra de linha no nome."""
    for mal in ("Conta 2", "conta2\n", "conta/2", "..", "x" * 33, "-abc", "_abc", ""):
        with pytest.raises(contas.ContaError) as e:
            contas.caminho(mal)
        assert e.value.status == 400
    assert contas.caminho("x" * 32) == casa / f".claude-{'x' * 32}"


def test_listar_devolve_so_conta_carimbada_e_real(casa):
    """Sem as guardas, um `~/.claude-evil -> fora` com marcador seria listado, e uma pasta ainda
    sendo semeada apareceria no seletor."""
    _carimbada(casa, "b")
    _carimbada(casa, "a")
    (casa / ".claude-backup").mkdir()
    fora = casa / "fora"
    fora.mkdir()
    (fora / contas.MARCADOR).write_text("", encoding="utf-8")
    marcador_link = casa / ".claude-marcador-link"
    marcador_link.mkdir()
    try:
        (casa / ".claude-evil").symlink_to(fora, target_is_directory=True)
        (marcador_link / contas.MARCADOR).symlink_to(fora / contas.MARCADOR)
    except OSError as exc:
        pytest.skip(f"Symlink indisponível nesta máquina: {exc}")
    (_carimbada(casa, "pendente") / ".hangar-account-pending").write_text("", encoding="utf-8")
    assert contas.listar() == ["a", "b"]


def test_reconciliar_passa_pelo_rust_e_devolve_os_avisos(casa, hangar_server_bin):
    _carimbada(casa, "conta2")
    (casa / ".claude" / "settings.json").write_text('{"enabledPlugins": {"sumido@x": true}}', encoding="utf-8")
    assert contas.reconciliar("conta2") == [
        "plugin sumido@x está ligado no settings.json mas não consta em installed_plugins.json — "
        "reinstalar: claude plugin install sumido@x"]
    assert (casa / ".claude-conta2" / "skills").is_symlink()
    assert _diario_do_rust(casa), "a reconciliação não passou pelo hangar-server conta"


def test_o_diario_do_comando_entra_no_arquivo_exportado(casa, hangar_server_bin, monkeypatch):
    """C191: o diário que o `hangar-server conta` grava (`diario/hangar-conta`) é lido pelo exportado."""
    import importlib.util
    from app import diag, log_paths
    # A suíte troca o `log_paths.base` pela pasta de teste; aqui vale o de verdade, que segue o HOME.
    spec = importlib.util.spec_from_file_location("log_paths_real", log_paths.__file__)
    real = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(real)
    monkeypatch.setattr(log_paths, "base", real.base)
    _carimbada(casa, "conta2")
    contas.reconciliar("conta2")
    assert _diario_do_rust(casa) and set(_diario_do_rust(casa)) <= set(diag.arquivos())


def test_falha_do_rust_vira_conta_error_com_a_frase_dele(casa, hangar_server_bin):
    with pytest.raises(contas.ContaError) as e:
        contas.reconciliar("naoexiste")
    assert (e.value.status, e.value.detail) == (500, f"{casa / '.claude-naoexiste'} não é uma conta criada pelo hangar")


def test_o_projeto_chega_ao_rust_como_veio_inclusive_vazio(casa, hangar_server_bin):
    _carimbada(casa, "conta2")
    with pytest.raises(contas.ContaError) as e:
        contas.reconciliar("conta2", "")
    assert (e.value.status, e.value.detail) == (500, "projeto inválido")
    assert contas.reconciliar("conta2", "-home-novo") == []
    assert (casa / ".claude-conta2" / "projects" / "-home-novo" / "memory").is_symlink()


def test_criar_devolve_o_caminho_da_conta_criada_pelo_rust(casa, hangar_server_bin):
    conta = contas.criar("conta3")
    assert conta == casa / ".claude-conta3" and contas.e_conta(conta)
    assert "oauthAccount" not in json.loads((conta / ".claude.json").read_text(encoding="utf-8"))
    assert _diario_do_rust(casa), "a criação não passou pelo hangar-server conta"


def test_sem_o_binario_a_conta_recusa_com_503(casa, monkeypatch):
    monkeypatch.setenv("CP_RUST_SERVER_BIN", str(casa / "nao-existe"))
    _carimbada(casa, "conta2")
    with pytest.raises(contas.ContaError) as e:
        contas.reconciliar("conta2")
    assert e.value.status == 503
    assert not (casa / ".claude-conta2" / "skills").exists()


def _lancador(casa: Path, binario: str, *args: str, flags: tuple[str, ...] = ()) -> subprocess.CompletedProcess:
    env = {"HOME": str(casa), "PATH": "/usr/bin:/bin", "LANG": "C.UTF-8", "CP_RUST_SERVER_BIN": binario}
    return subprocess.run([sys.executable, *flags, str(SCRIPT), *args], env=env, capture_output=True, text=True,
                          encoding="utf-8")


def test_o_lancador_repassa_saida_e_codigo_do_rust(casa, hangar_server_bin):
    _carimbada(casa, "conta2")
    feito = _lancador(casa, str(hangar_server_bin), "--prep", "conta2")
    assert (feito.returncode, feito.stdout, feito.stderr) == (0, f"{casa / '.claude-conta2'}\n", "")
    recusado = _lancador(casa, str(hangar_server_bin), "--prep", "Conta 2")
    assert (recusado.returncode, recusado.stderr) == (1, "hangar-conta: nome: use minúsculas, números, '-' ou '_' (até 32 caracteres)\n")
    assert _lancador(casa, str(hangar_server_bin), "--bogus").returncode == 2
    assert _diario_do_rust(casa)


def test_o_lancador_sem_binario_sai_126_com_o_python_do_sistema(casa):
    """Sem site-packages (`-S`), como o shell o chama: o lançador só importa o `app.rust_bins`."""
    sem = _lancador(casa, str(casa / "nao-existe"), "--list", flags=("-S",))
    assert sem.returncode == 126, sem.stderr
    assert sem.stderr.splitlines()[-1].startswith("hangar-conta: hangar-server não encontrado"), sem.stderr
