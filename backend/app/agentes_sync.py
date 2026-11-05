import os

from pathlib import Path

from app import atomico

def _codex_dir(home: Path | None) -> Path:
    if home:
        return home / ".codex"
    return Path(os.environ.get("CODEX_HOME") or (Path.home() / ".codex"))

def _escrever_restrito(alvo: Path, conteudo: str, modo: int) -> None:
    """Cria o arquivo JÁ com a permissão final e só então escreve.

    `write_text` seguido de `chmod` deixa uma JANELA: o arquivo nasce com o umask (0644 nesta
    máquina) e fica legível por qualquer usuário local enquanto a chave de API já está lá dentro.
    A janela é curta, mas o conteúdo é justo o que não pode vazar — e no caso mais comum (primeira
    sincronização, arquivo de destino ainda inexistente) ela acontece SEMPRE. `os.open` com o modo
    fecha isso na criação, que é o único ponto onde dá pra fechar.
    """
    fd = os.open(alvo, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, modo)
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as fh:
            fh.write(conteudo)
    except BaseException:
        alvo.unlink(missing_ok=True)
        raise
    # O umask pode ter apertado o modo na criação; o chmod aqui só AFROUXA de volta pro pedido,
    # nunca abre uma janela (o arquivo já existe com o conteúdo e permissão no máximo mais restrita).
    os.chmod(alvo, modo)

def _gravar_preservando(caminho: Path, conteudo: str) -> None:
    """tmp+rename preservando o modo do arquivo original (0600 tem que continuar 0600)."""
    modo = (caminho.stat().st_mode & 0o777) if caminho.exists() else 0o600
    tmp = caminho.with_name(f"{caminho.name}.tmp-hangar-{os.getpid()}")
    _escrever_restrito(tmp, conteudo, modo)
    atomico.substituir(tmp, caminho)
