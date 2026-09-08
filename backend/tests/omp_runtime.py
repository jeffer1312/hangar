"""Provas de extensões no OMP real, sem credenciais nem chamadas a modelos."""

import json
import os
from pathlib import Path
import shutil
import subprocess
import uuid


def _reusar_natives(home: Path) -> None:
    """Aponta o addon nativo da HOME de teste pro da máquina, quando existe.

    Sem isso o omp extrai ~344M por HOME (uma por caso), e as 3 rodadas que o pytest
    guarda lotam um /tmp em tmpfs no meio da suíte. Sem o addon na máquina (CI limpo),
    o omp extrai como sempre."""
    real = Path.home() / ".omp" / "natives"
    if not real.is_dir():
        return
    for alvo in (home / ".omp" / "natives", home / ".cache" / "omp" / "natives"):
        if alvo.exists() or alvo.is_symlink():
            continue
        alvo.parent.mkdir(parents=True, exist_ok=True)
        alvo.symlink_to(real, target_is_directory=True)


def run_omp_driver(
    driver: Path, home: Path, env: dict[str, str] | None = None, *, cwd: Path | None = None, load_rules: bool = False
) -> subprocess.CompletedProcess[str]:
    """Executa um driver isolado; exige conclusão estruturada além do código de saída."""
    binary = os.environ.get("OMP_TEST_BIN") or shutil.which("omp")
    if not binary:
        raise AssertionError("OMP ausente: instale o binário oficial ou defina OMP_TEST_BIN")
    home = home.resolve()
    home.mkdir(parents=True, exist_ok=True)
    agent_dir = home / ".omp" / "agent"
    agent_dir.mkdir(parents=True, exist_ok=True)
    config = agent_dir / "config.yml"
    if not config.exists():
        config.write_text("setupVersion: 2\n", encoding="utf-8")
    _reusar_natives(home)
    run_id = uuid.uuid4().hex
    proof = home / "provas" / run_id
    proof.mkdir(parents=True)
    result_path = proof / "resultado.json"
    # Allowlist: nem credenciais, Git, plugins ou tmux do chamador entram no processo.
    process_env = {key: os.environ[key] for key in ("PATH", "SYSTEMROOT", "WINDIR", "LANG") if key in os.environ}
    process_env.update(env or {})
    process_env.update({
        "HOME": str(home), "USERPROFILE": str(home),
        "XDG_CONFIG_HOME": str(home / ".config"),
        "XDG_DATA_HOME": str(home / ".local/share"),
        "XDG_CACHE_HOME": str(home / ".cache"),
        "PI_CODING_AGENT_DIR": str(agent_dir),
        "CLAUDE_CONFIG_DIR": str(home / ".claude"),
        "GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": os.devnull,
        "OMP_DRIVER_RESULT": str(result_path), "OMP_DRIVER_RUN": run_id,
    })
    command = [str(binary), "--no-extensions", "--no-skills", "--no-session", "--print",
               "--model", "openai-codex/gpt-6-astra", "--api-key", "fixture-sem-credencial",
               "--extension", str(driver.resolve())]
    if not load_rules:
        command.append("--no-rules")
    try:
        result = subprocess.run(command, cwd=cwd or home, env=process_env, input="", capture_output=True, text=True, encoding="utf-8", errors="replace", timeout=60)
    except subprocess.TimeoutExpired as exc:
        (proof / "stdout.log").write_bytes(exc.stdout or b"")
        (proof / "stderr.log").write_bytes(exc.stderr or b"")
        raise AssertionError(f"OMP excedeu o prazo; logs: {proof}") from exc
    (proof / "stdout.log").write_text(result.stdout, encoding="utf-8")
    (proof / "stderr.log").write_text(result.stderr, encoding="utf-8")
    assert result.returncode == 0, f"OMP falhou ({result.returncode}); logs: {proof}\n{result.stderr}"
    assert result_path.is_file(), f"Driver não concluiu; logs: {proof}\n{result.stderr}"
    payload = json.loads(result_path.read_text(encoding="utf-8"))
    assert payload.get("run") == run_id and payload.get("ok") is True, f"Driver falhou; logs: {proof}\n{payload}"
    return result
