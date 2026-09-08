"""Adota artefatos nativos e mescla configuração sem tomar entradas particulares."""
from __future__ import annotations

from copy import deepcopy
from pathlib import Path

from app.codex_msgs import msg
from app.codex_arquivos import AlteradoExternamente, backup, gravar, hash_bytes, ler


def reconciliar_arquivos(desejados: dict[Path, bytes], anteriores: dict, backups: Path,
                         *, confiaveis: set[Path] | None = None) -> tuple[dict, list[str]]:
    """Retorna manifesto ``path -> {hash}`` e avisos após escritas com backup e comparação.

    ``confiaveis`` contém somente paths cuja origem no importador nativo foi comprovada
    pelo chamador. O manifesto anterior deve vir do estado privado da integração.
    """
    fontes = {str(path): data for path, data in desejados.items()}
    historico = {str(path) for path in confiaveis or ()}
    manifesto, avisos = {}, []
    for nome, data in sorted(fontes.items()):
        path = Path(nome)
        anterior = anteriores.get(nome)
        esperado = anterior.get("hash") if isinstance(anterior, dict) else None
        try:
            for tentativa in range(3):
                atual = ler(path)
                if atual is not None and atual != data and esperado is None and nome not in historico:
                    avisos.append(msg("aviso_artefato_sem_proveniencia", path=path))
                    break
                try:
                    # Em artefatos gerenciados, a fonte Claude prevalece também sobre
                    # edições locais; cada tentativa relê os bytes antes da escrita.
                    gravar(path, data, atual, backups)
                    manifesto[nome] = {"hash": hash_bytes(data)}
                    break
                except AlteradoExternamente:
                    if tentativa == 2:
                        raise
        except (OSError, ValueError, AlteradoExternamente) as exc:
            avisos.append(msg("aviso_artefato_falha", path=path, erro=exc))
            if esperado is not None:
                manifesto[nome] = deepcopy(anterior)
    for nome, anterior in sorted(anteriores.items()):
        if nome in fontes or not isinstance(anterior, dict) or not isinstance(anterior.get("hash"), str):
            continue
        path = Path(nome)
        try:
            atual = ler(path)
            if atual is None:
                continue
            if hash_bytes(atual) != anterior["hash"]:
                avisos.append(msg("aviso_artefato_obsoleto_alterado", path=path))
                manifesto[nome] = deepcopy(anterior)
                continue
            backup(path, atual, backups)
            if ler(path) != atual:
                raise AlteradoExternamente(f"Artefato alterado durante a retirada: {path}")
            path.unlink()
        except (OSError, ValueError, AlteradoExternamente) as exc:
            avisos.append(msg("aviso_artefato_obsoleto_falha", path=path, erro=exc))
            manifesto[nome] = deepcopy(anterior)
    return manifesto, avisos


_AUSENTE = object()


def _tirar_gerenciado(atual, anterior):
    """Remove recursivamente só os valores ainda iguais aos registrados anteriormente."""
    if atual == anterior:
        return _AUSENTE
    if not isinstance(atual, dict) or not isinstance(anterior, dict):
        return deepcopy(atual)
    novo = deepcopy(atual)
    for chave, velho in anterior.items():
        if chave not in novo:
            continue
        restante = _tirar_gerenciado(novo[chave], velho)
        if restante is _AUSENTE:
            novo.pop(chave)
        else:
            novo[chave] = restante
    return novo if novo else _AUSENTE


def _mesclar_campos(atual, fonte, anterior):
    """A fonte define seus campos; complementos particulares ficam na configuração."""
    if not isinstance(fonte, dict):
        return deepcopy(fonte)
    novo = deepcopy(atual) if isinstance(atual, dict) else {}
    velhos = anterior if isinstance(anterior, dict) else {}
    for chave, velho in velhos.items():
        if chave in fonte or chave not in novo:
            continue
        resto = _tirar_gerenciado(novo[chave], velho)
        if resto is _AUSENTE:
            novo.pop(chave)
        else:
            novo[chave] = resto
    for chave, valor in fonte.items():
        novo[chave] = _mesclar_campos(novo.get(chave), valor, velhos.get(chave))
    return novo


def mesclar_config(atual: dict, fonte: dict, anteriores: dict,
                   *, confiaveis: set[str] | None = None) -> tuple[dict, dict, list[str]]:
    """Mescla uma seção MCP ou agents e retorna configuração, manifesto e avisos.

    O manifesto guarda apenas os valores vindos da fonte, por nome de entrada; jamais
    os campos complementares particulares que aparecerem na configuração resultante.
    """
    if not all(isinstance(data, dict) for data in (atual, fonte, anteriores)):
        raise ValueError("Configuração e manifestos precisam ser objetos")
    novo, manifesto, avisos = deepcopy(atual), {}, []
    historico = confiaveis or set()
    for nome, valor in fonte.items():
        if nome in atual and nome not in anteriores and nome not in historico and atual[nome] != valor:
            avisos.append(msg("aviso_config_sem_proveniencia", nome=nome))
            continue
        novo[nome] = _mesclar_campos(atual.get(nome), valor, anteriores.get(nome))
        manifesto[nome] = deepcopy(valor)
    for nome, velho in anteriores.items():
        if nome in fonte or nome not in atual:
            continue
        restante = _tirar_gerenciado(atual[nome], velho)
        if restante is _AUSENTE:
            novo.pop(nome, None)
        else:
            novo[nome] = restante
            avisos.append(msg("aviso_config_campos_particulares", nome=nome))
    return novo, manifesto, avisos
