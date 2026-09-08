"""OmpAdapter: o oh-my-pi e um fork do Pi com o mesmo JSONL e a mesma API de extensao.

Subclasse, nao copia: parser, layout e entrega vem do Pi. O que difere e binario, raiz de
sessoes e o par spawn/resume — o omp nao tem `--session-id`, entao o app escolhe o CAMINHO do
transcript (`--session`) e exporta CP_PI_SESSION pro fallback do bilhete.

Perfil (`omp --profile x`) vai como OMP_PROFILE no ambiente do pane, nao como flag: o omp honra
a variavel, e e dela que o registry le o perfil de um pane vivo (procinfo._env_var_of), o mesmo
caminho por onde ele le CP_ENGINE e CLAUDE_CONFIG_DIR.
"""
import os

from app import model_args
from app.adapters.pi import sessions as pi_sessions
from app.adapters.pi.adapter import PiAdapter


def _com_env(session_id: str, cmd: list[str], perfil: str | None = None) -> list[str]:
    # `env` porque o tmux nao repassa o ambiente do caller ao pane; ele faz exec, entao o argv0
    # que o registry le continua sendo `omp`. No Windows nao ha `env` — fica so o bilhete.
    if os.name == "nt":
        return cmd
    variaveis = [f"CP_PI_SESSION={session_id}"] + ([f"OMP_PROFILE={perfil}"] if perfil else [])
    return ["env", *variaveis] + cmd


class OmpAdapter(PiAdapter):
    provider = "omp"

    def spawn_command(self, cwd: str, session_id: str,
                      model: str | None = None, effort: str | None = None,
                      permission_mode: str | None = None, perfil: str | None = None) -> list[str]:
        alvo = pi_sessions.transcript_alvo(cwd, session_id, "omp", perfil)
        return _com_env(session_id, ["omp", "--session", alvo] + model_args.args_de("omp", model, effort), perfil)

    def resume_command(self, cwd: str, session_id: str,
                       model: str | None = None, effort: str | None = None,
                       perfil: str | None = None) -> list[str]:
        caminho = pi_sessions.transcript_path(cwd, session_id, "omp", perfil)
        if not caminho:
            raise ValueError("transcript do omp nao encontrado para retomar")
        return _com_env(session_id, ["omp", "-r", caminho] + model_args.args_de("omp", model, effort), perfil)
