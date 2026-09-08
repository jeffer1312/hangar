"""Reconcilia skills com proveniência, sem assumir propriedade pelo nome."""
from __future__ import annotations

import json
import ntpath
import os
from pathlib import Path
import re
import tempfile

from app import skill_bridge
from app.codex_msgs import msg
from app.codex_arquivos import AlteradoExternamente, backup, gravar, hash_bytes, ler


def _caminho_comparavel(valor: str, *, windows: bool) -> str:
    """Normaliza só a comparação, preservando o destino original usado nas operações."""
    if not windows:
        return os.path.normpath(valor)
    # readlink no Windows usa nomes estendidos também para symlinks criados com C:\…
    # e para compartilhamentos UNC. Remover apenas o prefixo simples de UNC criaria
    # um caminho relativo incorreto, por isso ele tem conversão própria.
    if valor.upper().startswith("\\\\?\\UNC\\"):
        valor = "\\\\" + valor[8:]
    elif valor.startswith("\\\\?\\"):
        valor = valor[4:]
    return ntpath.normcase(ntpath.normpath(valor))


def _link_gerenciado(entrada: Path, raizes: tuple[str, ...], *, windows: bool | None = None) -> bool:
    if not entrada.is_symlink():
        return False
    windows = os.name == "nt" if windows is None else windows
    paths = ntpath if windows else os.path
    alvo = os.readlink(entrada)
    if not paths.isabs(alvo):
        alvo = paths.join(str(entrada.parent), alvo)
    norm = _caminho_comparavel(alvo, windows=windows)
    for raiz in raizes:
        base = _caminho_comparavel(raiz, windows=windows)
        try:
            if paths.commonpath((norm, base)) == base:
                return True
        except ValueError:
            # Unidades diferentes (ou mistura absoluto/relativo) não compartilham fonte.
            continue
    return False


def _arquivos(raiz: Path) -> dict[str, bytes]:
    arquivos = {}
    for atual, dirs, nomes in os.walk(raiz):
        for nome in dirs:
            if (Path(atual) / nome).is_symlink():
                raise ValueError(f"Subdiretório ligado externamente: {Path(atual) / nome}")
        for nome in nomes:
            path = Path(atual) / nome
            arquivos[path.relative_to(raiz).as_posix()] = path.read_bytes()
    return arquivos


def _nomes_skill(path: Path) -> set[str]:
    texto = (path / "SKILL.md").read_text(encoding="utf-8")
    if not texto.strip():
        return set()
    nomes = {path.name}
    if texto.startswith("---\n"):
        cabecalho = texto.split("\n---", 1)[0]
        match = re.search(r"(?m)^name:\s*['\"]?([\w.:-]+)['\"]?\s*$", cabecalho)
        if match:
            nomes.add(match[1])
    return nomes


def _nativas(plugins: dict, avisos: list[str]) -> list[tuple[str, set[str]]]:
    resultado = []
    for id_, plugin in plugins.items():
        try:
            raiz = Path(plugin["path"])
            declaradas = None
            for rel in (".codex-plugin/plugin.json", ".claude-plugin/plugin.json"):
                manifest = raiz / rel
                if manifest.is_file():
                    data = json.loads(manifest.read_text(encoding="utf-8"))
                    if not isinstance(data, dict):
                        raise ValueError("Manifesto de plugin não é objeto")
                    declaradas = data.get("skills")
                    break
            roots = ["skills"] if declaradas is None else declaradas
            if isinstance(roots, str):
                roots = [roots]
            if not isinstance(roots, list) or any(not isinstance(r, str) for r in roots):
                raise ValueError("Raízes de skills inválidas")
            for rel in roots:
                caminho = Path(rel)
                if caminho.is_absolute() or ".." in caminho.parts:
                    raise ValueError("Raiz de skill fora do plugin")
                root = raiz / caminho
                skills = [root] if (root / "SKILL.md").is_file() else skill_bridge._skills_em(root)
                for skill in skills:
                    nomes = _nomes_skill(skill)
                    if nomes:
                        resultado.append((id_, nomes))
        except (OSError, ValueError, KeyError, TypeError) as exc:
            avisos.append(msg("aviso_plugin_skills", id=id_, erro=exc))
    return resultado


def _mesmo_plugin(origem: Path, id_: str, home: Path) -> bool:
    nome, separador, mercado = id_.rpartition("@")
    if not separador or not nome or not mercado:
        return False
    real = origem.resolve()
    cache = home / ".claude/plugins/cache" / mercado / nome
    if real.is_relative_to(cache.resolve()):
        return True
    marketplace = (home / ".claude/plugins/marketplaces" / mercado).resolve()
    if not real.is_relative_to(marketplace):
        return False
    # Em marketplaces locais, o plugin pode morar diretamente na raiz ou numa subpasta.
    for pai in (real, *real.parents):
        if not pai.is_relative_to(marketplace):
            break
        manifest = pai / ".claude-plugin/plugin.json"
        if manifest.is_file():
            try:
                return json.loads(manifest.read_text(encoding="utf-8")).get("name") == nome
            except (OSError, ValueError, AttributeError):
                return False
    return False


def _estado_anterior(anterior: dict, destino: Path) -> dict:
    if not isinstance(anterior, dict) or not anterior or anterior.get("path", str(destino)) != str(destino):
        return {}
    return {
        **anterior,
        "mode": anterior.get("mode", {"link": "symlink", "copia": "copy"}.get(anterior.get("tipo"))),
        "files": anterior.get("files", anterior.get("arquivos", {})),
    }


def _seguro(destino: Path, rel: str) -> Path:
    sub = Path(rel)
    if sub.is_absolute() or ".." in sub.parts:
        raise ValueError("Caminho inválido no manifesto de skills")
    path = destino / sub
    for pai in (path, *path.parents):
        if pai == destino:
            break
        if pai.is_symlink():
            raise ValueError(f"Arquivo da skill virou link: {pai}")
    return path


def _copiar(destino: Path, origem: Path, anterior: dict, backups: Path, avisos: list[str]) -> dict:
    arquivos = _arquivos(origem)
    velhos = anterior.get("files", {})
    gerenciados = {}
    destino.mkdir(parents=True, exist_ok=True)
    for rel, data in arquivos.items():
        try:
            path = _seguro(destino, rel)
            for tentativa in range(3):
                atual = ler(path)
                if atual is not None and rel not in velhos:
                    avisos.append(msg("aviso_arquivo_exclusivo", path=path))
                    break
                try:
                    # O Claude define o conteúdo gerenciado. A releitura a cada tentativa
                    # evita aplicar uma escrita calculada sobre bytes já substituídos.
                    gravar(path, data, atual, backups)
                    gerenciados[rel] = hash_bytes(data)
                    break
                except AlteradoExternamente:
                    if tentativa == 2:
                        raise
        except (OSError, ValueError, AlteradoExternamente) as exc:
            avisos.append(msg("aviso_arquivo_falha", path=destino / rel, erro=exc))
            if rel in velhos:
                gerenciados[rel] = velhos[rel]
    for rel, hash_anterior in velhos.items():
        if rel in arquivos:
            continue
        try:
            path = _seguro(destino, rel)
            atual = ler(path)
            if atual is None:
                continue
            if hash_bytes(atual) != hash_anterior:
                avisos.append(msg("aviso_arquivo_removido_alterado", path=path))
                gerenciados[rel] = hash_anterior
                continue
            backup(path, atual, backups)
            if ler(path) != atual:
                raise ValueError(f"Arquivo alterado durante a retirada: {path}")
            path.unlink()
        except (OSError, ValueError) as exc:
            avisos.append(msg("aviso_arquivo_falha", path=destino / rel, erro=exc))
            gerenciados[rel] = hash_anterior
    return {"path": str(destino), "mode": "copy", "source": str(origem), "files": gerenciados}


def _retirar_copia(destino: Path, anterior: dict, backups: Path, avisos: list[str]) -> dict:
    """Retira só bytes reconhecidos; conteúdo exclusivo ou editado continua no lugar."""
    restantes = {}
    for rel, hash_anterior in anterior.get("files", {}).items():
        path = _seguro(destino, rel)
        atual = ler(path)
        if atual is None:
            continue
        if hash_bytes(atual) != hash_anterior:
            restantes[rel] = hash_anterior
            avisos.append(msg("aviso_ponte_alteracao_local", path=path))
            continue
        backup(path, atual, backups)
        if ler(path) != atual:
            raise ValueError(f"Arquivo alterado durante a retirada: {path}")
        path.unlink()
    for atual, _, _ in os.walk(destino, topdown=False):
        path = Path(atual)
        if not path.is_symlink() and not any(path.iterdir()):
            path.rmdir()
    if destino.exists():
        avisos.append(msg("aviso_ponte_conteudo_pessoal", destino=destino))
    return {**anterior, "files": restantes}


def _trocar_link(destino: Path, origem: Path, backups: Path, esperado: str | None) -> None:
    fd, nome = tempfile.mkstemp(prefix=f".{destino.name}.hangar-", dir=destino.parent)
    os.close(fd)
    tmp = Path(nome)
    tmp.unlink()
    try:
        tmp.symlink_to(origem, target_is_directory=True)
        if os.path.lexists(destino):
            if not destino.is_symlink() or os.readlink(destino) != esperado:
                raise FileExistsError(f"Entrada deixou de ser link: {destino}")
            backup(destino, os.readlink(destino).encode(), backups)
        atual = os.readlink(destino) if destino.is_symlink() else None
        if atual != esperado or (os.path.lexists(destino) and not destino.is_symlink()):
            raise FileExistsError(f"Entrada alterada durante a troca: {destino}")
        os.replace(tmp, destino)
    finally:
        tmp.unlink(missing_ok=True)


def _duplicata_nativa(nome: str, origem: Path, home: Path, anterior: dict,
                      backups: Path, avisos: list[str]) -> None:
    destino = home / ".agents/skills" / nome
    if not os.path.lexists(destino) or destino.resolve() == origem.resolve():
        return
    if destino.is_symlink() or not destino.is_dir():
        avisos.append(msg("aviso_skill_nativa_sem_proveniencia", destino=destino))
        return
    atuais = _arquivos(destino)
    hashes = {rel: hash_bytes(data) for rel, data in atuais.items()}
    historico = _estado_anterior(anterior, destino)
    comprovada = (historico.get("mode") == "copy" and historico.get("files") == hashes
                  and historico.get("source", historico.get("origem")) == str(origem))
    # ~/.agents/skills é fonte do Pi, do Kimi e do omp: só sai o que o Hangar mesmo pôs lá.
    if not comprovada:
        avisos.append(msg("aviso_skill_duplicata_plugin", nome=nome))
        return
    # Confere novamente depois de gravar os backups e antes de retirar qualquer arquivo.
    for rel, data in atuais.items():
        backup(destino / rel, data, backups)
    if _arquivos(destino) != atuais:
        raise ValueError(f"Skill nativa alterada durante a reconciliação: {destino}")
    for rel in atuais:
        (destino / rel).unlink()
    for atual, dirs, _ in os.walk(destino, topdown=False):
        for nome_dir in dirs:
            (Path(atual) / nome_dir).rmdir()
    destino.rmdir()


def reconciliar_skills(home: Path, codex_home: Path, plugins: dict, registro_skills: dict,
                       backups: Path, *, windows: bool = False) -> tuple[dict, list[str]]:
    """Retorna manifesto por nome e avisos; plugins recebidos devem estar habilitados."""
    avisos: list[str] = []
    fontes = skill_bridge._varrer_fontes(home)
    nativas = _nativas(plugins, avisos)
    ponte = codex_home / "skills"
    ponte.mkdir(parents=True, exist_ok=True)
    raizes = skill_bridge._raizes(home)
    manifesto = {}
    for nome, origem in sorted(fontes.items()):
        if nome == ".system":
            continue
        destino = ponte / nome
        anterior = _estado_anterior(registro_skills.get(nome, {}), destino)
        try:
            nomes = _nomes_skill(origem)
            nativa = next((id_ for id_, disponiveis in nativas
                           if nomes & disponiveis and _mesmo_plugin(origem, id_, home)), None)
            ja_nativa = origem.resolve().is_relative_to((home / ".agents/skills").resolve())
            # O Codex lê ~/.agents/skills sozinho: skill que já está lá não ganha link na ponte.
            em_agents = not ja_nativa and os.path.lexists(home / ".agents/skills" / nome)
            if nativa or ja_nativa or em_agents:
                if _link_gerenciado(destino, raizes):
                    backup(destino, os.readlink(destino).encode(), backups)
                    destino.unlink()
                manifesto[nome] = {"path": str(destino), "mode": "native", "source": str(origem),
                                   "plugin": nativa, "files": {}}
                if not destino.is_symlink() and anterior.get("mode") == "copy":
                    resto = _retirar_copia(destino, anterior, backups, avisos)
                    if destino.exists():
                        manifesto[nome] = resto
                if nativa:
                    _duplicata_nativa(nome, origem, home, registro_skills.get(nome, {}), backups, avisos)
                elif em_agents and (home / ".agents/skills" / nome).is_dir() and _arquivos(home / ".agents/skills" / nome) != _arquivos(origem):
                    avisos.append(msg("aviso_skill_agents_difere", nome=nome))
                continue
            if os.path.lexists(destino):
                if destino.is_symlink():
                    if not _link_gerenciado(destino, raizes):
                        avisos.append(msg("aviso_link_pessoal", destino=destino))
                        continue
                    if destino.resolve() == origem.resolve():
                        manifesto[nome] = {"path": str(destino), "mode": "symlink", "source": str(origem), "files": {}}
                        continue
                elif anterior.get("mode") != "copy":
                    avisos.append(msg("aviso_skill_pessoal", destino=destino))
                    continue
                else:
                    manifesto[nome] = _copiar(destino, origem, anterior, backups, avisos)
                    continue
            try:
                esperado = os.readlink(destino) if destino.is_symlink() else None
                _trocar_link(destino, origem, backups, esperado)
                manifesto[nome] = {"path": str(destino), "mode": "symlink", "source": str(origem), "files": {}}
            except OSError:
                if not windows or os.path.lexists(destino):
                    raise
                manifesto[nome] = _copiar(destino, origem, anterior, backups, avisos)
        except (OSError, ValueError) as exc:
            avisos.append(msg("aviso_skill_falha", nome=nome, erro=exc))
            if nome not in manifesto and anterior:
                manifesto[nome] = anterior
    # Fontes vazias podem significar instalação temporariamente indisponível.
    if fontes:
        for destino in ponte.iterdir():
            if destino.name != ".system" and destino.name not in fontes and _link_gerenciado(destino, raizes):
                backup(destino, os.readlink(destino).encode(), backups)
                destino.unlink()
        for nome, anterior in registro_skills.items():
            if Path(nome).name != nome or nome in {".", ".."}:
                avisos.append(msg("aviso_skill_nome_invalido", nome=nome))
                continue
            destino = ponte / nome
            estado = _estado_anterior(anterior, destino)
            if (nome not in fontes and nome != ".system" and estado.get("mode") == "copy"
                    and destino.is_dir() and not destino.is_symlink()):
                try:
                    resto = _retirar_copia(destino, estado, backups, avisos)
                    if destino.exists():
                        manifesto[nome] = resto
                except (OSError, ValueError) as exc:
                    manifesto[nome] = estado
                    avisos.append(msg("aviso_copia_obsoleta_falha", nome=nome, erro=exc))
    else:
        manifesto = dict(registro_skills)
        avisos.append(msg("aviso_skills_sem_fonte"))
    return manifesto, avisos
