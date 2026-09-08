"""Um reconciliador para instalação, atualização e reparo do Codex, sem depender do Desktop."""
from __future__ import annotations

import asyncio
import copy
import contextlib
import hashlib
import json
import logging
import os
import re
import shutil
import sys
import tempfile
import time
import tomllib
from datetime import datetime, timezone
from pathlib import Path
from urllib.parse import urlsplit

from app.codex_arquivos import (
    AlteradoExternamente, backup, exclusivo, gravar, hash_bytes, json_bytes,
    json_obj, ler, mesclar_hooks, remapear, transformar,
)
from app.codex_compat import normalizar_hooks, normalizar_security_guidance, remover_instrucao_de_leitura, wrapper_instalado
from app.codex_instrucoes import limite_instrucoes, preparar_instrucoes
from app.codex_importador import CodexNativo, CodexNativoErro
from app.codex_msgs import msg, serializar

_log = logging.getLogger("hangar.codex.integracao")
_REPO = Path(__file__).resolve().parents[2]
_INTERVALO = 6 * 60 * 60
_IMPORTAVEIS = {"CONFIG", "HOOKS", "MCP_SERVER_CONFIG", "COMMANDS", "SUBAGENTS"}
_ID = re.compile(r"^[A-Za-z0-9_.-]+@[A-Za-z0-9_.-]+$")
# Espelho que o instalador antigo (install-skills-bridge.sh) deixava do hooks.json que ELE escreveu.
_ESPELHO_ANTIGO = ".hangar-hooks.json"
# Rodada parcial/erro só é refeita depois disto (a abertura seguinte de sessão a dispara).
_RETENTATIVA = 300


def sincronizacao_ligada() -> bool:
    """Gatilhos automáticos (laço do backend e abertura de TUI). O botão do painel não passa aqui."""
    if os.environ.get("CP_CODEX_SYNC_ENABLED", "1").lower() in ("0", "false", "no"):
        return False
    from app import runtime_config
    from app.config import automations_enabled
    return bool(runtime_config.get("codex_sync")) and automations_enabled()


def _e_hook_do_app(command: object) -> bool:
    """Hook do próprio Hangar (backend/hooks/): cada harness recebe o seu pelo instalador dele
    (codex_hook_installer aqui), então ele não atravessa pelo importador do Codex."""
    return isinstance(command, str) and "backend/hooks/" in command.replace("\\", "/")


def sem_hooks_do_app(hooks: dict) -> dict:
    if not isinstance(hooks, dict):
        return hooks
    out = {}
    for evento, grupos in hooks.items():
        if not isinstance(grupos, list):
            out[evento] = grupos
            continue
        restantes = []
        for grupo in grupos:
            if not isinstance(grupo, dict) or not isinstance(grupo.get("hooks"), list):
                restantes.append(grupo)
                continue
            proprios = [h for h in grupo["hooks"] if not (isinstance(h, dict) and _e_hook_do_app(h.get("command")))]
            if proprios or not grupo["hooks"]:
                restantes.append({**grupo, "hooks": proprios})
        if restantes:
            out[evento] = restantes
    return out


def _iso(tempo: float | None) -> str | None:
    return datetime.fromtimestamp(tempo, timezone.utc).isoformat() if tempo else None


def _snapshot() -> dict:
    return {"estado": "ocioso", "etapa": "", "ultima_execucao": None,
            "proxima_atualizacao": None, "plugins": [], "erros": [], "avisos": [],
            "confianca_pendente": False}


def _toml(path: Path) -> dict:
    raw = ler(path)
    return tomllib.loads(raw.decode("utf-8")) if raw is not None else {}


def _chave(*partes: str) -> str:
    return ".".join(json.dumps(p, ensure_ascii=False) for p in partes)


def _plugins_desejados(settings: dict) -> set[str]:
    enabled = settings.get("enabledPlugins", {})
    if (not isinstance(enabled, dict) or any(
            not isinstance(k, str) or not _ID.fullmatch(k) or not isinstance(v, bool)
            for k, v in enabled.items())):
        raise ValueError("enabledPlugins inválido; instalações existentes preservadas")
    return {k for k, v in enabled.items() if v}


def _origem_marketplace(data: dict, *, claude: bool = False) -> tuple[str, str] | None:
    source = data.get("source", {}) if claude else data
    if not isinstance(source, dict):
        return None
    tipo = source.get("source" if claude else "source_type")
    if tipo == "github":
        origem = "https://github.com/" + source.get("repo", "")
        tipo = "git"
    else:
        origem = (source.get("url") or source.get("path")) if claude else source.get("source")
    if not isinstance(origem, str) or not origem:
        return None
    if tipo in ("directory", "local"):
        path = os.path.normcase(str(Path(origem).resolve())).removeprefix("\\\\?\\")
        return "local", path
    if tipo == "git":
        # SSH e HTTPS do mesmo repositório identificam a mesma origem; não guarda credenciais.
        if origem.startswith("git@") and ":" in origem:
            host, repo = origem[4:].split(":", 1)
        else:
            url = urlsplit(origem)
            host, repo = url.hostname, url.path.lstrip("/")
        if host:
            return "git", f"{host.lower()}/{repo.rstrip('/').removesuffix('.git')}"
    return None


def _identidade_plugin(id_: str, origem: tuple[str, str], mercados: dict, inventario: dict) -> str:
    """O mesmo repositório pode declarar nomes distintos nos manifestos de cada harness."""
    nome, mercado = id_.rsplit("@", 1)
    if mercado in mercados:
        if _origem_marketplace(mercados[mercado]) != origem:
            raise ValueError("Marketplace homônimo de outra origem")
        if id_ in inventario:
            return id_
    aliases = [p for p in inventario if p.rsplit("@", 1)[0] == nome and
               _origem_marketplace(mercados.get(p.rsplit("@", 1)[1], {})) == origem]
    if len(aliases) > 1:
        raise ValueError("Mais de uma identidade nativa para o plugin")
    return aliases[0] if aliases else id_


class IntegracaoCodex:
    def __init__(self, home: Path | None = None, codex_home: Path | None = None,
                 *, nativo=CodexNativo, binario: str = "codex"):
        self._home, self._codex_home = home, codex_home
        self.nativo, self.binario = nativo, binario
        self._task: asyncio.Task | None = None
        self._estado: dict | None = None
        self._plugins_confirmados: set[str] = set()

    @property
    def home(self) -> Path:
        return (self._home or Path.home()).absolute()

    @property
    def codex_home(self) -> Path:
        return (self._codex_home or Path(os.environ.get("CODEX_HOME") or self.home / ".codex")).absolute()

    @property
    def raiz(self) -> Path:
        key = hashlib.sha256(str(self.codex_home.resolve()).encode()).hexdigest()[:24]
        return self.home / ".hangar" / "codex-integracao" / key

    @property
    def backups(self) -> Path:
        return self.raiz / "backups"

    def status(self) -> dict:
        estado = self._status()
        estado["skills"] = self._resumo_skills()
        # Código + parâmetros pra tela traduzir; o texto vai junto pro log e pro lançador.
        estado["etapa"] = serializar(estado.get("etapa"))
        estado["avisos"] = [serializar(a) for a in estado.get("avisos", [])]
        estado["erros"] = [serializar(e) for e in estado.get("erros", [])]
        return estado

    def _status(self) -> dict:
        if self._estado is not None and self._estado.get("estado") == "executando":
            return copy.deepcopy(self._estado)
        try:
            salvo = json_obj(self.raiz / "estado.json")
            estado = {**_snapshot(), **salvo.get("status", {})}
            if estado["estado"] == "executando":
                estado.update(estado="ocioso", etapa=msg("etapa_reinicio"))
            if self._estado is not None and (self._estado.get("ultima_execucao") or "") >= (estado.get("ultima_execucao") or ""):
                return copy.deepcopy(self._estado)
            return estado
        except (OSError, ValueError):
            return {**_snapshot(), "estado": "erro", "erros": [msg("erro_registro_ilegivel")]}

    def _resumo_skills(self) -> dict:
        """O card mostra isto no lugar do item "ponte de skills" que a ponte antiga tinha."""
        try:
            skills = json_obj(self.raiz / "estado.json").get("skills", {})
        except (OSError, ValueError):
            return {"ponte": 0, "nativas": 0}
        modos = [d.get("mode") for d in skills.values() if isinstance(d, dict)]
        return {"ponte": sum(m in ("symlink", "copy") for m in modos), "nativas": modos.count("native")}

    async def iniciar(self, motivo: str = "manual", forcar: bool = True) -> dict:
        if self._task is None or self._task.done():
            self._estado = {**self.status(), "estado": "executando", "etapa": msg("etapa_aguardando")}
            self._task = asyncio.create_task(self.reconciliar(motivo, forcar))
        return self.status()

    async def fechar(self) -> None:
        if self._task and not self._task.done():
            self._task.cancel()
            try:
                await self._task
            except asyncio.CancelledError:
                pass

    async def _mutacao(self, fn, *args):
        """Cancelamento espera a escrita terminar antes de persistir/liberar o lock."""
        task = asyncio.create_task(asyncio.to_thread(fn, *args))
        try:
            return await asyncio.shield(task)
        except asyncio.CancelledError:
            with contextlib.suppress(Exception):
                await task
            raise

    def _checkpoint(self, registro: dict) -> None:
        registro["status"] = self.status()  # serializado: o código da mensagem sobrevive a uma queda no meio
        path = self.raiz / "estado.json"
        gravar(path, json_bytes(registro), ler(path))

    def _normalizar(self, data: dict, wrapper: tuple[str, str] | None = None) -> dict:
        python, script = wrapper or (sys.executable, str(_REPO / "scripts" / "codex-hook-allow.py"))
        return normalizar_hooks(data, python, Path(script), windows=os.name == "nt")

    def _etapa(self, texto: str) -> None:
        self._estado["etapa"] = texto

    def _erro(self, texto: str) -> None:
        self._estado["erros"].append(texto)
        _log.warning("%s", texto)

    def _confianca(self) -> None:
        self._estado["confianca_pendente"] = True
        aviso = msg("aviso_hooks_alterados")
        if aviso not in self._estado["avisos"]:
            self._estado["avisos"].append(aviso)

    async def reconciliar(self, motivo: str = "manual", forcar: bool = False) -> dict:
        anterior = self.status()
        self._estado = {**_snapshot(), "estado": "executando", "etapa": msg("etapa_inventariando"),
                        "plugins": anterior["plugins"], "ultima_execucao": anterior["ultima_execucao"],
                        "confianca_pendente": anterior["confianca_pendente"]}
        if not (self.home / ".claude" / "settings.json").is_file():
            self._estado.update(estado="indisponivel", etapa=msg("etapa_sem_claude"))
            return self.status()
        if self.nativo is CodexNativo and not shutil.which(self.binario):
            self._estado.update(estado="indisponivel", etapa=msg("etapa_sem_codex"))
            return self.status()
        registro = {}
        self._plugins_confirmados = set()
        lock = exclusivo(self.codex_home / ".hangar-integracao.lock")
        adquirido = carregado = False
        fonte_inicial = None
        try:
            fonte_inicial = await asyncio.to_thread(self.fingerprint, fontes=True)
            await lock.__aenter__()
            adquirido = True
            self.raiz.mkdir(parents=True, exist_ok=True, mode=0o700)
            registro = json_obj(self.raiz / "estado.json")
            carregado = True
            self.codex_home.mkdir(parents=True, exist_ok=True)
            settings = json_obj(self.home / ".claude" / "settings.json")
            desejados = _plugins_desejados(settings)
            self._etapa(msg("etapa_instrucoes"))
            await self._mutacao(self._instrucoes)
            await self._mutacao(self._migrar_ponte_antiga)
            await self._mutacao(self._hooks, {}, registro)
            async with self.nativo(self.home, self.codex_home, self.binario) as codex:
                await self._config(codex, {}, {})
                self._etapa(msg("etapa_importando"))
                await self._plugins(codex, desejados, registro, forcar)
                self._etapa(msg("etapa_fragmentos"))
                await self._fragmentos(codex, settings, registro)
                # DEPOIS da importação: é ela que reescreve os comandos pra `<codex>/hooks/` e
                # decide o que copiar. Antes dela não há o que materializar.
                await self._mutacao(self._hooks_arquivos)
                self._checkpoint(registro)
                self._etapa(msg("etapa_skills"))
                await self._mutacao(self._skills, registro)
                self._checkpoint(registro)
                await self._conferir_confianca(codex)
            self._estado["estado"] = "parcial" if self._estado["erros"] else "ok"
        except asyncio.CancelledError:
            self._estado.update(estado="ocioso", etapa=msg("etapa_interrompida"))
            raise
        except (OSError, ValueError, RuntimeError) as exc:
            # Não devolve conteúdo de config ou saídas de subprocessos que podem conter chaves.
            self._erro(msg("erro_falha", tipo=type(exc).__name__))
            self._estado["estado"] = "erro"
            _log.debug("Falha da integração", exc_info=True)
        except Exception as exc:  # noqa: BLE001 — erro escapando deixava o estado preso em "executando"
            self._erro(msg("erro_falha_inesperada", tipo=type(exc).__name__))
            self._estado["estado"] = "erro"
            _log.exception("Falha inesperada da integração")
        finally:
            if carregado:
                self._estado["ultima_execucao"] = _iso(time.time())
                if self._estado["estado"] != "ocioso":
                    self._estado["etapa"] = msg("etapa_concluido") if self._estado["estado"] == "ok" else msg("etapa_pendencias")
                registro["status"] = self.status()
                # Fonte que mudou no meio da rodada não vira assinatura: a próxima abertura relê.
                try:
                    registro["fingerprint"] = (self.fingerprint() if fonte_inicial == self.fingerprint(fontes=True) else None)
                except OSError:
                    registro["fingerprint"] = None
                try:
                    path = self.raiz / "estado.json"
                    gravar(path, json_bytes(registro), ler(path))
                except (OSError, ValueError, RuntimeError):
                    _log.exception("Não foi possível persistir o estado da integração")
            if adquirido:
                await lock.__aexit__(None, None, None)
        return self.status()

    def _instrucoes(self) -> None:
        preparar_instrucoes(self.home, self.codex_home)
        alvo = self.codex_home / 'AGENTS.md'
        if alvo.is_file() and not alvo.is_symlink():
            transformar(alvo, lambda raw: remover_instrucao_de_leitura(raw.decode()).encode(), self.backups)

    async def _conferir_confianca(self, codex) -> None:
        try:
            dados = await codex.request("hooks/list", {"cwds": [str(self.home)]})
            entradas = dados.get("data") if isinstance(dados, dict) else None
            if not isinstance(entradas, list) or any(not isinstance(e, dict) or not isinstance(e.get("hooks", []), list)
                                                     or any(not isinstance(h, dict) for h in e.get("hooks", []))
                                                     for e in entradas):
                raise CodexNativoErro("hooks/list em formato desconhecido")
            pendente = any(h.get("enabled") and h.get("trustStatus") in ("untrusted", "modified")
                          for entry in entradas for h in entry.get("hooks", []))
            if pendente:
                self._confianca()
            else:
                self._estado["confianca_pendente"] = False
                self._estado["avisos"] = [a for a in self._estado["avisos"]
                                          if getattr(a, "codigo", None) != "aviso_hooks_alterados"]
        except CodexNativoErro:
            self._estado["avisos"].append(msg("aviso_confianca_indisponivel"))

    def _hooks(self, fonte: dict, registro: dict) -> None:
        path = self.codex_home / "hooks.json"
        anteriores = registro.get("hooks", {})
        normalizada = fonte

        def atualizar(raw):
            nonlocal normalizada
            atual = json.loads(raw) if raw else {}
            wrapper = wrapper_instalado(atual, windows=os.name == "nt")
            atual = self._normalizar(atual, wrapper)
            normalizada = self._normalizar(fonte, wrapper)
            merged = mesclar_hooks(atual, normalizada, anteriores) if normalizada else atual
            return raw if raw and json.loads(raw) == merged else json_bytes(merged)

        if ler(path) is not None or fonte.get("hooks"):
            if transformar(path, atualizar, self.backups):
                self._confianca()
        if fonte:
            registro["hooks"] = normalizada

    def _hooks_arquivos(self) -> None:
        """O importador nativo reescreve o comando pra `<codex>/hooks/x` e copia o arquivo — mas
        pula symlink (medido em 07/09/2026 no codex-cli 0.153.4). Quem versiona hooks num repo e
        linka em `~/.claude/hooks` ficava com o comando apontando pro vazio, e hook de PreToolUse
        que falha BLOQUEIA a ferramenta. Aqui o que faltou vira symlink pro mesmo alvo."""
        from app import codex_hooks_arquivos
        criados, orfaos = codex_hooks_arquivos.materializar(self.codex_home, self.home)
        if criados:
            _log.info("codex: hooks materializados em ~/.codex/hooks: %s", ", ".join(criados))
        if orfaos:
            # Sem equivalente no Claude: o app não tem de onde tirar o arquivo, e o hook vai falhar
            # na próxima sessão. Dizer isso é melhor que a pessoa descobrir por um "no such file"
            # no meio de um turno.
            self._estado["avisos"].append(msg("aviso_hooks_sem_arquivo", arquivos=", ".join(orfaos)))

    def _migrar_ponte_antiga(self) -> None:
        """Primeira rodada numa máquina onde o instalador antigo escreveu o hooks.json: o que ele
        pôs lá sai (o importador traz tudo de novo, senão cada hook roda em dobro) e o espelho some."""
        espelho = self.codex_home / _ESPELHO_ANTIGO
        raw_espelho = ler(espelho)
        if raw_espelho is None:
            return
        antigo = json.loads(raw_espelho)
        path = self.codex_home / "hooks.json"

        def atualizar(raw):
            atual = json.loads(raw) if raw else {}
            merged = mesclar_hooks(atual, {"hooks": {}}, antigo)
            return raw if raw and json.loads(raw) == merged else json_bytes(merged)

        if ler(path) is not None and transformar(path, atualizar, self.backups):
            self._confianca()
        backup(espelho, raw_espelho, self.backups)
        espelho.unlink()
        # O instalador da subida já rodou e viu a entrada antiga; sem isto a sessão fica sem hook
        # de estado até o próximo restart do backend.
        from app.codex_hook_installer import ensure_codex_state_hook_installed
        ensure_codex_state_hook_installed(self.codex_home)

    async def _editar_config(self, preparar) -> None:
        """Escritor TOML oficial numa cópia; cada tentativa recalcula sobre o arquivo atual."""
        path = self.codex_home / "config.toml"
        for _ in range(3):
            raw = ler(path)
            atual = tomllib.loads(raw.decode()) if raw else {}
            edits, confirmar = preparar(atual)
            if not edits:
                confirmar()
                return
            with tempfile.TemporaryDirectory(prefix="hangar-config-", dir=self.raiz) as temp:
                temp_home = Path(temp)
                (temp_home / ".codex").mkdir()
                copia = temp_home / ".codex" / "config.toml"
                gravar(copia, raw or b"", None)
                async with self.nativo(temp_home, temp_home / ".codex", self.binario) as writer:
                    await writer.request("config/batchWrite", {"edits": edits, "reloadUserConfig": False})
                try:
                    gravar(path, copia.read_bytes(), raw, self.backups)
                    confirmar()
                    return
                except AlteradoExternamente:
                    continue
        raise AlteradoExternamente("config.toml continua mudando; nova leitura necessária")

    async def _config(self, codex, mcp: dict, agentes: dict, *, hooks: bool = False,
                      registro: dict | None = None, historico: dict | None = None,
                      env: dict | None = None) -> None:
        from app.codex_fragmentos import mesclar_config
        def preparar(atual):
            fallbacks = atual.get("project_doc_fallback_filenames", [])
            if not isinstance(fallbacks, list):
                raise ValueError("project_doc_fallback_filenames inválido")
            nomes = list(dict.fromkeys([*fallbacks, "CLAUDE.md", "CLAUDE.MD"]))
            edits = []
            limite = atual.get("project_doc_max_bytes", 32768)
            if not isinstance(limite, int) or isinstance(limite, bool) or limite < 0:
                raise ValueError("project_doc_max_bytes inválido")
            necessario = limite_instrucoes(self.codex_home)
            if limite < necessario:
                edits.append({"keyPath": "project_doc_max_bytes", "value": necessario, "mergeStrategy": "replace"})
            if nomes != fallbacks:
                edits.append({"keyPath": "project_doc_fallback_filenames", "value": nomes, "mergeStrategy": "replace"})
            if hooks and atual.get("features", {}).get("hooks") is not True:
                edits.append({"keyPath": "features.hooks", "value": True, "mergeStrategy": "replace"})
            manifestos, avisos = {}, []
            if registro is not None:
                for secao, valores in (("mcp_servers", mcp), ("agents", agentes)):
                    antes = atual.get(secao, {})
                    novo, manifesto, pendencias = mesclar_config(
                        antes, valores, registro.get(secao, {}),
                        confiaveis=(historico or {}).get(secao, set()),
                    )
                    manifestos[secao] = manifesto
                    avisos.extend(pendencias)
                    for nome in sorted(set(antes) | set(novo)):
                        if antes.get(nome) != novo.get(nome):
                            edits.append({"keyPath": _chave(secao, nome), "value": novo.get(nome), "mergeStrategy": "replace"})
                if env is not None:
                    antes = atual.get("shell_environment_policy", {}).get("set", {})
                    novo, manifesto, pendencias = mesclar_config(antes, env, registro.get("env", {}))
                    manifestos["env"] = manifesto
                    avisos.extend(pendencias)
                    for nome in sorted(set(antes) | set(novo)):
                        if antes.get(nome) != novo.get(nome):
                            edits.append({"keyPath": _chave("shell_environment_policy", "set", nome),
                                          "value": novo.get(nome), "mergeStrategy": "replace"})
            def confirmar():
                if registro is not None:
                    registro.update(manifestos)
                self._estado["avisos"].extend(avisos)
            return edits, confirmar
        await self._editar_config(preparar)

    async def _plugins(self, codex, desejados: set[str], registro: dict, forcar: bool) -> None:
        for tentativa in range(3):
            raw = ler(self.home / ".claude/settings.json")
            if raw is None:
                raise ValueError("Fonte do Claude desapareceu durante a integração")
            desejados = _plugins_desejados(json.loads(raw))
            try:
                await self._reconciliar_plugins(codex, desejados, registro, forcar, raw)
                return
            except AlteradoExternamente:
                if tentativa == 2:
                    raise

    async def _reconciliar_plugins(self, codex, desejados, registro, forcar, fonte_raw):
        anteriores = copy.deepcopy(registro.get("plugins", {}))
        cfg_path = self.codex_home / "config.toml"
        raw = ler(cfg_path)
        if raw is not None:
            backup(cfg_path, raw, self.backups)
        conhecidos = json_obj(self.home / ".claude/plugins/known_marketplaces.json")
        mercados = _toml(cfg_path).get("marketplaces", {})
        bloqueados = set()
        for id_ in desejados:
            mercado = id_.rsplit("@", 1)[1]
            origem = _origem_marketplace(conhecidos.get(mercado, {}), claude=True)
            destino = _origem_marketplace(mercados.get(mercado, {}))
            if origem is None or (mercado in mercados and origem != destino):
                bloqueados.add(id_)
                self._erro(msg("erro_marketplace_origem", marketplace=mercado))
        candidatos = desejados - bloqueados
        # Recorta a seleção nativa pela identidade completa, inclusive em marketplaces homônimos.
        itens = []
        for item in await codex.detectar():
            if item.get("itemType") != "PLUGINS":
                continue
            item = copy.deepcopy(item)
            grupos = []
            for grupo in item.get("details", {}).get("plugins", []):
                nomes = [n for n in grupo.get("pluginNames", []) if f"{n}@{grupo.get('marketplaceName')}" in candidatos]
                if nomes:
                    grupos.append({**grupo, "pluginNames": nomes})
            if grupos:
                item["details"]["plugins"] = grupos
                itens.append(item)
        if itens:
            result = await codex.importar(itens)
            for tipo in result.get("itemTypeResults", []):
                if tipo.get("failures"):
                    self._erro(msg("erro_plugins_incompletos"))
        mercados = _toml(cfg_path).get("marketplaces", {})
        inventario = {p["pluginId"]: p for p in await codex.plugins_instalados()}
        identidades = {}
        for id_ in sorted(candidatos):
            try:
                origem = _origem_marketplace(conhecidos[id_.rsplit("@", 1)[1]], claude=True)
                identidades[id_] = _identidade_plugin(id_, origem, mercados, inventario)
            except ValueError:
                bloqueados.add(id_)
                self._erro(msg("erro_plugin", id=id_))
        candidatos -= bloqueados
        agora = time.time()
        ultima = max(registro.get("marketplaces_em", 0), registro.get("marketplaces_tentativa_em", 0))
        falhas_anteriores = set(registro.get("marketplaces_pendentes", []))
        atualizar = forcar or agora - ultima >= _INTERVALO or bool(falhas_anteriores and agora - ultima >= 300)
        falhas = set()
        if atualizar:
            for marketplace in sorted({identidades[p].rsplit("@", 1)[1] for p in candidatos}):
                source = _toml(cfg_path).get("marketplaces", {}).get(marketplace, {})
                if source.get("source_type") != "git":
                    continue
                try:
                    self._etapa(msg("etapa_marketplace", marketplace=marketplace))
                    result = await codex.atualizar_marketplace(marketplace)
                    if result.get("errors"):
                        raise CodexNativoErro("Falha ao atualizar marketplace")
                except (OSError, ValueError, RuntimeError):
                    self._erro(msg("erro_marketplace_atualizar", marketplace=marketplace))
                    falhas.add(marketplace)
            registro["marketplaces_tentativa_em"] = agora
            registro["marketplaces_pendentes"] = sorted(falhas)
            if not falhas:
                registro["marketplaces_em"] = agora
        else:
            for marketplace in sorted(falhas_anteriores):
                self._erro(msg("erro_marketplace_pendente", marketplace=marketplace))
        inventario = {p["pluginId"]: p for p in await codex.plugins_instalados()}
        plugins_pendentes = set(registro.get("plugins_pendentes", [])) & desejados
        plugins = {p: anteriores[p] for p in bloqueados if p in anteriores}
        for id_ in sorted(candidatos):
            try:
                id_codex = identidades[id_]
                conhecido = anteriores.get(id_)
                instalado = inventario.get(id_codex)
                if (atualizar or id_ in plugins_pendentes or not conhecido or not instalado or
                        conhecido.get("id_codex", id_) != id_codex or
                        instalado.get("version") != conhecido.get("versao") or
                        not Path(conhecido.get("path", "")).is_dir()):
                    self._etapa(msg("etapa_plugin", id=id_))
                    data = await codex.instalar_plugin(id_codex)
                    conhecido = {"path": data["installedPath"], "versao": data.get("version", ""),
                                 "origem": id_codex.rsplit("@", 1)[1], "id_codex": id_codex}
                plugins[id_] = conhecido
                # Retém a identidade mesmo se uma etapa posterior falhar ou a fonte mudar.
                registro.setdefault("plugins", {})[id_] = conhecido
                self._hooks_plugin(Path(conhecido["path"]))
                plugins_pendentes.discard(id_)
                self._checkpoint(registro)
            except (OSError, KeyError, ValueError, RuntimeError):
                self._erro(msg("erro_plugin", id=id_))
                plugins_pendentes.add(id_)
                if id_ in anteriores:
                    plugins[id_] = anteriores[id_]
        if ler(self.home / ".claude/settings.json") != fonte_raw:
            raise AlteradoExternamente("Plugins do Claude mudaram durante a integração")
        # Só desabilita identidades anteriormente adotadas, nunca plugins exclusivos do Codex.
        removidos = set(anteriores) - desejados
        ativos = {d.get("id_codex", p) for p, d in plugins.items()}
        if removidos:
            await self._habilitar_plugins(codex, {anteriores[p].get("id_codex", p): False for p in removidos
                                                if anteriores[p].get("id_codex", p) not in ativos})
        await self._habilitar_plugins(codex, {d.get("id_codex", p): True for p, d in plugins.items() if p in candidatos})
        registro["plugins"] = plugins
        registro["plugins_pendentes"] = sorted(plugins_pendentes)
        self._plugins_confirmados = set(plugins) & candidatos
        self._estado["plugins"] = [{"id": d.get("id_codex", p), "versao": d["versao"], "origem": d["origem"]} for p, d in plugins.items()]
        self._estado["proxima_atualizacao"] = _iso(max(registro.get("marketplaces_em", 0), registro.get("marketplaces_tentativa_em", agora)) + (300 if registro.get("marketplaces_pendentes") else _INTERVALO))

    async def _habilitar_plugins(self, codex, escolhas: dict[str, bool]) -> None:
        def preparar(atual):
            edits = [{"keyPath": _chave("plugins", p, "enabled"), "value": enabled, "mergeStrategy": "replace"}
                     for p, enabled in escolhas.items() if atual.get("plugins", {}).get(p, {}).get("enabled") != enabled]
            return edits, lambda: None
        await self._editar_config(preparar)

    def _hooks_plugin(self, raiz: Path) -> None:
        cache = self.codex_home / "plugins" / "cache"
        if not raiz.resolve().is_relative_to(cache.resolve()):
            raise ValueError("Plugin fora do cache do Codex")
        paths = {raiz / "hooks" / "hooks.json", raiz / "hooks.json"}
        security_guidance = False
        for rel in (".codex-plugin/plugin.json", ".claude-plugin/plugin.json"):
            manifest = raiz / rel
            if not manifest.is_file():
                continue
            dados = json_obj(manifest)
            security_guidance |= dados.get("name") == "security-guidance"
            declaradas = dados.get("hooks")
            if isinstance(declaradas, dict):
                paths.add(manifest)
            else:
                refs = [declaradas] if isinstance(declaradas, str) else declaradas or []
                if not isinstance(refs, list) or any(not isinstance(r, str) for r in refs):
                    raise ValueError("Referências de hooks inválidas no plugin")
                paths.update(raiz / r for r in refs)
        wrapper_json = self.codex_home / ".hangar-hooks" / "codex-hook-json.py"
        if security_guidance:
            gravar(wrapper_json, (_REPO / "scripts/codex-hook-json.py").read_bytes(), ler(wrapper_json), self.backups)
        for path in sorted(paths):
            if not path.is_file():
                continue
            if not path.resolve().is_relative_to(raiz.resolve()):
                raise ValueError("Hooks fora do plugin gerenciado")
            def converter(raw):
                data = json.loads(raw)
                result = self._normalizar(data)
                if security_guidance:
                    result = normalizar_security_guidance(result, sys.executable, wrapper_json, windows=os.name == "nt")
                return raw if data == result else json_bytes(result)
            if transformar(path, converter, self.backups):
                self._confianca()

    async def _historico(self, codex) -> dict:
        result = {"mcp_servers": set(), "agents": set(), "commands": set()}
        try:
            histories = await codex.historicos_importacao()
        except CodexNativoErro:
            self._estado["avisos"].append(msg("aviso_historico_indisponivel"))
            return result
        tipos = {"MCP_SERVER_CONFIG": "mcp_servers", "SUBAGENTS": "agents", "COMMANDS": "commands"}
        for entry in histories:
            if entry.get("providerId") not in (None, "claude-code"):
                continue
            for item in entry.get("successes", []):
                tipo, nome = tipos.get(item.get("itemType")), item.get("target")
                if tipo and isinstance(nome, str) and nome and Path(nome).name == nome and nome not in (".", "..") and item.get("cwd") is None:
                    result[tipo].add(nome)
        return result

    async def _fragmentos(self, codex, settings: dict, registro: dict) -> None:
        for tentativa in range(3):
            try:
                await self._importar_fragmentos(codex, registro)
                return
            except AlteradoExternamente:
                if tentativa == 2:
                    raise

    async def _importar_fragmentos(self, codex, registro: dict) -> None:
        from app.codex_fragmentos import reconciliar_arquivos
        historico = await self._historico(codex)
        inicio = self.fingerprint(fontes=True)
        settings_raw = ler(self.home / ".claude" / "settings.json")
        if settings_raw is None:
            raise ValueError("Fonte do Claude indisponível")
        settings = json.loads(settings_raw)
        with tempfile.TemporaryDirectory(prefix="import-", dir=self.raiz) as temp:
            stage = Path(temp)
            cc, cx = stage / ".claude", stage / ".codex"
            cc.mkdir()
            cx.mkdir()
            # Somente fontes de ferramentas; autenticação do agente e preferências ficam fora.
            env = settings.get("env", {})
            if not isinstance(env, dict) or any(
                    not isinstance(k, str) or not k or "=" in k or "\0" in k or
                    not isinstance(v, str) or "\0" in v for k, v in env.items()):
                raise ValueError("settings.env inválido; variáveis existentes preservadas")
            config = {"hooks": sem_hooks_do_app(settings.get("hooks", {})), "env": env}
            # O detector nativo ignora alguns shapes inválidos; isso nunca significa remoção.
            mesclar_hooks({}, config, {})
            gravar(cc / "settings.json", json_bytes(config), None)
            for nome in ("commands", "agents"):
                origem = self.home / ".claude" / nome
                if origem.is_dir():
                    shutil.copytree(origem, cc / nome, ignore=shutil.ignore_patterns("__pycache__", "*.pyc", ".git"))
            source_mcp = self.home / ".claude.json"
            mcp_raw = ler(source_mcp)
            if mcp_raw is not None:
                mcp = json_obj(source_mcp).get("mcpServers", {})
                if not isinstance(mcp, dict) or any(not isinstance(v, dict) for v in mcp.values()):
                    raise ValueError("Configuração MCP inválida; servidores existentes preservados")
                for value in mcp.values():
                    if (not isinstance(value.get("command", value.get("url")), str) or
                            ("args" in value and (not isinstance(value["args"], list) or any(not isinstance(a, str) for a in value["args"]))) or
                            ("env" in value and (not isinstance(value["env"], dict) or any(not isinstance(v, str) for v in value["env"].values())))):
                        raise ValueError("Entrada MCP inválida; servidores existentes preservados")
                gravar(stage / ".claude.json", json_bytes({"mcpServers": mcp}), None)
            congelados: set[str] = set()
            async with self.nativo(stage, cx, self.binario) as importer:
                itens = [i for i in await importer.detectar() if i.get("itemType") in _IMPORTAVEIS]
                for pasta, tipo, detalhe in (("agents", "SUBAGENTS", "subagents"), ("commands", "COMMANDS", "commands")):
                    nomes = {e.get("name", "") for i in itens if i.get("itemType") == tipo
                             for e in i.get("details", {}).get(detalhe, []) if isinstance(e, dict)}
                    # Um README.md em ~/.claude/agents não é agente: fica de fora com aviso, e o que já
                    # tinha sido importado com esse nome é preservado — sem derrubar a etapa inteira.
                    ignorados = [md for md in sorted((cc / pasta).rglob("*.md"))
                                 if not any(md.stem == n or md.stem in n for n in nomes)]
                    if ignorados:
                        congelados.update(md.stem for md in ignorados)
                        self._estado["avisos"].append(msg(
                            "aviso_ignorados", pasta=pasta,
                            arquivos=", ".join(str(md.relative_to(cc / pasta)) for md in ignorados)))
                if itens:
                    result = await importer.importar(itens)
                    if any(r.get("failures") for r in result.get("itemTypeResults", [])):
                        raise CodexNativoErro("Importação de fragmentos incompleta")
            def remap(value):
                # CODEX_HOME personalizado não precisa ser filho do HOME real.
                return remapear(remapear(value, cx, self.codex_home), stage, self.home)
            native_cfg = remap(_toml(cx / "config.toml"))
            native_env = native_cfg.get("shell_environment_policy", {}).get("set", {})
            if native_env != env:
                raise ValueError("Importação nativa de env incompleta; variáveis existentes preservadas")
            hooks = remap(json_obj(cx / "hooks.json"))
            if "hooks" not in hooks:
                hooks = {"hooks": {}}
            if self.fingerprint(fontes=True) != inicio:
                raise AlteradoExternamente("Fontes do Claude mudaram durante a importação")
            desejados, confiaveis = {}, set()
            for src_root, dst_root in ((cx / "agents", self.codex_home / "agents"),
                                       (stage / ".agents" / "skills", self.home / ".agents" / "skills")):
                if not src_root.is_dir():
                    continue
                for src in sorted(src_root.rglob("*")):
                    if not src.is_file():
                        continue
                    dst = dst_root / src.relative_to(src_root)
                    data = src.read_bytes()
                    try:
                        data = remap(data.decode()).encode()
                    except UnicodeDecodeError:
                        pass
                    desejados[dst] = data
                    if src_root == cx / "agents":
                        if src.stem in historico["agents"]:
                            confiaveis.add(dst)
                    elif src.relative_to(src_root).parts[0] in historico["commands"]:
                        confiaveis.add(dst)
            anteriores = registro.get("artefatos", {})
            guardados = {k: v for k, v in anteriores.items() if Path(k).stem in congelados}
            manifesto, avisos = reconciliar_arquivos(
                desejados, {k: v for k, v in anteriores.items() if k not in guardados},
                self.backups, confiaveis=confiaveis,
            )
            manifesto.update(guardados)
            registro["artefatos"] = manifesto
            self._estado["avisos"].extend(avisos)
            self._checkpoint(registro)
            self._hooks(hooks, registro)
            # Não vincula um agente cujo arquivo colidiu com conteúdo exclusivo do Codex.
            agentes = native_cfg.get("agents", {})
            agentes = {n: v for n, v in agentes.items() if not isinstance(v, dict) or
                       not v.get("config_file") or str(v["config_file"]) in manifesto}
            await self._config(codex, native_cfg.get("mcp_servers", {}), agentes,
                               hooks=native_cfg.get("features", {}).get("hooks") is True,
                               registro=registro, historico=historico, env=native_env)

    def _skills(self, registro: dict) -> None:
        from app.codex_skills import reconciliar_skills
        habilitados = _toml(self.codex_home / "config.toml").get("plugins", {})
        manifesto, avisos = reconciliar_skills(
            self.home, self.codex_home,
            {p: d for p, d in registro.get("plugins", {}).items() if p in self._plugins_confirmados and
             habilitados.get(d.get("id_codex", p), {}).get("enabled") is True},
            registro.get("skills", {}), self.backups, windows=os.name == "nt",
        )
        registro["skills"] = manifesto
        self._estado["avisos"].extend(avisos)

    def fingerprint(self, *, fontes: bool = False) -> str:
        h = hashlib.sha256()
        h.update(b"instrucoes-nativas-v1")
        caminhos = [self.home / ".claude" / "settings.json", self.home / ".claude.json"]
        caminhos.extend(self.home / ".claude" / nome for nome in ("CLAUDE.md", "CLAUDE.MD"))
        for path in (self.codex_home / ".hangar-instrucoes").glob('*.json'):
            dados = json_obj(path)
            caminhos.extend([path, Path(dados['fonte']), Path(dados['alvo'])])
        if not fontes:
            caminhos.extend([self.codex_home / "hooks.json", self.codex_home / "AGENTS.md",
                             self.codex_home / "config.toml", self.codex_home / "plugins" / "installed_plugins.json"])
            caminhos.append(_REPO / "scripts/codex-hook-json.py")
            caminhos.append(self.codex_home / ".hangar-hooks/codex-hook-json.py")
        caminhos.extend([self.home / ".claude/plugins/installed_plugins.json",
                         self.home / ".claude/plugins/known_marketplaces.json"])
        from app import skill_bridge
        roots = {self.home / ".claude/commands", self.home / ".claude/agents"}
        roots.update(p.resolve() for p in skill_bridge._varrer_fontes(self.home).values())
        roots.update({self.home / ".claude/skills", self.home / ".agents/skills", _REPO / "skills"})
        for raiz in sorted(roots):
            caminhos.append(raiz)
            def falhou(exc):
                if not isinstance(exc, FileNotFoundError):
                    raise exc
            for atual, dirs, nomes in os.walk(raiz, onerror=falhou):
                dirs[:] = [d for d in dirs if d not in ("__pycache__", ".git")]
                nomes = [n for n in nomes if not n.endswith(".pyc")]
                caminhos.extend(Path(atual) / nome for nome in [*dirs, *nomes])
        for path in sorted(caminhos):
            try:
                st = path.stat()
                h.update(f"{path}:{st.st_size}:{st.st_mtime_ns}".encode())
            except FileNotFoundError:
                h.update(f"{path}:ausente".encode())
        return h.hexdigest()

    def precisa_reconciliar(self) -> bool:
        """Cache por conteúdo: fonte igual à da última rodada, marketplace dentro do prazo e sem
        falha a retentar = não há o que fazer, e abrir sessão não chama o Codex."""
        if self._task and not self._task.done():
            return False
        try:
            registro = json_obj(self.raiz / "estado.json")
        except (OSError, ValueError):
            return True
        if registro.get("fingerprint") != self.fingerprint():
            return True
        status = registro.get("status", {})
        proxima = status.get("proxima_atualizacao")
        if proxima and datetime.fromisoformat(proxima).timestamp() <= time.time():
            return True
        ultima = status.get("ultima_execucao")
        if not ultima or status.get("estado") == "ocioso":
            return True  # nunca rodou, ou foi interrompida no meio
        if status.get("estado") in ("parcial", "erro"):
            return datetime.fromisoformat(ultima).timestamp() + _RETENTATIVA <= time.time()
        return False

    async def sessao(self) -> dict:
        """Gatilho da abertura de uma sessão Codex: dispara e devolve na hora; quem espera é o lançador."""
        if not sincronizacao_ligada() or not await asyncio.to_thread(self.precisa_reconciliar):
            return self.status()
        return await self.iniciar("sessao", False)


SERVICO = IntegracaoCodex()
