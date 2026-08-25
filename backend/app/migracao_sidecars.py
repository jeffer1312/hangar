"""Renomeia os sidecars `.claude-pocket-*` pra `.hangar-*` e deixa um link no caminho antigo.

O app se chama hangar; o nome `claude-pocket` só sobrevivia nos diretórios de dados. O código
agora conhece SÓ o nome novo — quem faz a ponte é este módulo, chamado na subida do backend
(`app/main.py`), que é o único momento garantido de acontecer numa máquina que acabou de puxar o
código novo (rodar os `install-*.sh` não é garantido; reiniciar o serviço é).

O link no caminho antigo é o que impede a máquina de se partir no meio da atualização: hook,
extensão do Pi e publicador de statusline que estejam VIVOS (ou desatualizados, como o
`~/.kimi-code/statusline.js`, que nem mora neste repo) continuam escrevendo no nome velho e caem
na pasta nova. Sem ele, quem grava e quem lê param de se encontrar — em silêncio, que é o modo de
falha que esta migração existe pra evitar.

Duas coisas que o desenho decide de propósito:

- **Nunca funde duas pastas.** Se o destino já existe e a origem também é real (não um link
  nosso), a migração PARA naquele item e avisa. Mesclar às cegas escolheria um vencedor por
  arquivo e perderia estado sem deixar rastro.
- **Falhar aqui não pode derrubar o backend.** Cada item vai no seu próprio try: uma pasta sem
  permissão custa aquele sidecar, não a subida do servidor.
"""

import logging
import os
import shutil
import subprocess
from pathlib import Path

_log = logging.getLogger("hangar.migracao_sidecars")

_ANTIGO = ".claude-pocket-"
_NOVO = ".hangar-"


def nome_novo(antigo: Path) -> Path:
    """`.claude-pocket-pair` -> `.hangar-pair`. Vale igual pro `.json` solto."""
    return antigo.with_name(antigo.name.replace(_ANTIGO, _NOVO, 1))


def caminho_de_leitura(novo: Path) -> Path:
    """O caminho a LER: o novo; o antigo só quando o novo ainda não existe.

    Existe pelos `.json` SOLTOS (apelidos, conn, models, runner, opencode). Pra pasta, o link que
    `migrar_caminho` deixa pra trás resolve tudo — mas link de ARQUIVO não dá pra criar no Windows
    sem privilégio, então lá o arquivo antigo pode continuar sendo o único que existe (a migração
    falhou, ou a máquina veio de um backup). Ler os dois é mais barato que exigir privilégio.

    Escrita NUNCA passa por aqui: grava-se sempre no nome novo.
    """
    if novo.exists():
        return novo
    antigo = novo.with_name(novo.name.replace(_NOVO, _ANTIGO, 1))
    return antigo if antigo.exists() else novo


def _link_compat(antigo: Path, novo: Path) -> bool:
    """Aponta o caminho antigo pro novo. True se conseguiu.

    No Windows `os.symlink` exige Modo de Desenvolvedor ou admin; pra DIRETÓRIO existe a junção
    (`mklink /J`), que não exige nada. Pra ARQUIVO não existe equivalente (link fixo se desfaz no
    primeiro tmp+rename), e é por isso que os leitores dos `.json` soltos leem os dois caminhos.
    """
    try:
        os.symlink(novo, antigo, target_is_directory=novo.is_dir())
        return True
    except (OSError, NotImplementedError, AttributeError):
        pass
    if os.name == "nt" and novo.is_dir():
        # `cmd` resolvido pelo caminho absoluto (mesma régua de todo subprocess do backend): nome
        # cru no argv obedece ao PATH de quem subiu o serviço.
        exe = shutil.which("cmd")
        if not exe:
            return False
        try:
            r = subprocess.run([exe, "/c", "mklink", "/J", str(antigo), str(novo)],
                               capture_output=True, timeout=10)
            return r.returncode == 0
        except (OSError, subprocess.SubprocessError):
            pass
    return False


def migrar_caminho(antigo: Path, novo: Path) -> bool:
    """Renomeia `antigo` -> `novo` e deixa link no lugar. True se migrou agora.

    Idempotente: caminho antigo ausente, ou já sendo um link, não faz nada.
    """
    try:
        if not antigo.exists() and not antigo.is_symlink():
            return False
        if antigo.is_symlink():
            return False                      # já é a nossa ponte
        if novo.exists() or novo.is_symlink():
            _log.warning("migracao: %s e %s existem os dois — deixando como estao", antigo, novo)
            return False
        antigo.rename(novo)
    except OSError as e:
        _log.warning("migracao: nao consegui renomear %s -> %s: %s", antigo, novo, e)
        return False
    if not _link_compat(antigo, novo):
        _log.warning("migracao: %s renomeado, mas sem link no caminho antigo (%s)", novo, antigo)
    return True


def migrar_base(base: Path) -> int:
    """Migra todos os `.claude-pocket-*` de UM diretório de configuração. Devolve quantos foram."""
    try:
        antigos = sorted(base.glob(_ANTIGO + "*"))
    except OSError:
        return 0
    return sum(migrar_caminho(a, nome_novo(a)) for a in antigos)


def migrar(bases) -> int:
    """Migra cada diretório de configuração recebido (todos os perfis `~/.claude*`)."""
    total = 0
    for base in {Path(b) for b in bases}:
        total += migrar_base(base)
    if total:
        _log.info("migracao: %d sidecar(s) renomeado(s) de .claude-pocket-* pra .hangar-*", total)
    return total
