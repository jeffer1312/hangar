"""Transporte de prova: os testes originais exercitam as interfaces Rust de produção."""
from dataclasses import asdict, dataclass, is_dataclass
import json
import logging
import os
from pathlib import Path
import subprocess


class ContractError(ValueError):
    pass


class ConflictError(ContractError):
    pass


def _json(value):
    if is_dataclass(value):
        return asdict(value)
    if hasattr(value,"isoformat"): return value.isoformat()
    if isinstance(value, Path):
        return str(value)
    if hasattr(value, "model_dump"):
        return value.model_dump()
    raise TypeError(type(value).__name__)


def call(action, **arguments):
    root = Path(__file__).resolve().parents[2]
    binary = Path(os.environ.get("HANGAR_ORQ_CONTRACT_BIN") or root / "crates" / "target" / "debug" / "examples" / ("orq_contract.exe" if os.name == "nt" else "orq_contract"))
    if not binary.is_file():
        raise RuntimeError("compile hangar-server --example orq_contract antes desta prova")
    result = subprocess.run([str(binary)], input=json.dumps({"action": action, "input": arguments}, default=_json, ensure_ascii=False),
                            capture_output=True, text=True, encoding="utf-8", timeout=15)
    if result.returncode:
        raise RuntimeError(f"orq_contract encerrou com {result.returncode}: {result.stderr}")
    if result.stderr.strip(): logging.getLogger("orq_contract").warning("%s", result.stderr.strip())
    response = json.loads(result.stdout)
    if not response["ok"]:
        error = response["error"]
        raise (ConflictError if "mudou desde" in error else ContractError)(error)
    return response["result"]


class Markdown:
    Conflito = ConflictError

    @staticmethod
    def normalizar(celula): return call("markdown.normalizar", celula=celula)
    @staticmethod
    def limpar(celula): return call("markdown.limpar", celula=celula)
    @staticmethod
    def validar_celula(valor): return call("markdown.validar_celula", valor=valor)
    @staticmethod
    def ler_tabela(texto, cabecalho): return call("markdown.ler_tabela", texto=texto, cabecalho=cabecalho)
    @staticmethod
    def trocar_linha(texto, cabecalho, chave, valores, titulo_se_ausente):
        return call("markdown.trocar_linha", texto=texto, cabecalho=cabecalho, chave=chave, valores=valores, titulo_se_ausente=titulo_se_ausente)
    @staticmethod
    def remover_linha(texto, cabecalho, chave): return call("markdown.remover_linha", texto=texto, cabecalho=cabecalho, chave=chave)
    @staticmethod
    def inserir_coluna(texto, cabecalho, nome, em, valor="-"):
        return call("markdown.inserir_coluna", texto=texto, cabecalho=cabecalho, nome=nome, em=em, valor=valor)
    @staticmethod
    def trocar_secao(texto, titulo, corpo): return call("markdown.trocar_secao", texto=texto, titulo=titulo, corpo=corpo)
    @staticmethod
    def ler_arquivo(path): return tuple(call("file.read", path=path))
    @staticmethod
    def gravar(path, texto, mtime_lido=None): return call("file.write", path=path, texto=texto, mtime=mtime_lido)


@dataclass(frozen=True)
class Role:
    papel: str
    sessao: str
    provider: str
    conta: str
    modelo: str
    esforco: str
    vez: str = ""
    headless: bool | None = None
    permissao: str = ""
    motor: str = ""
    jev: bool = False
    subagente: str = ""
    perfil: str = ""
    abertura_extra: str = ""
    janela: str = ""

    def e_arbitro(self): return call("markdown.normalizar", celula=self.papel) == "arbitro"


class Roles:
    Papel = Role
    @staticmethod
    def ler(texto): return [Role(**row) for row in call("roles.ler", texto=texto)]
    @staticmethod
    def escrever_papel(texto, p): return call("roles.escrever_papel", texto=texto, p=p)
    @staticmethod
    def abertura_texto(p): return call("roles.abertura_texto", p=p)
    @staticmethod
    def _ler_abertura(text):
        record="| papel | sessão | provider | conta | modelo | esforço | abertura |\n|---|---|---|---|---|---|---|\n| executor | test | claude | default | - | - | "+text+" |\n"
        return call("roles.ler",texto=record)[0]
    @staticmethod
    def tem_coluna_vez(texto): return "vez" in (call("roles.cabecalho_atual", texto=texto) or [])
    @staticmethod
    def casar_viva(papel, sessoes): return call("roles.casar_viva", papel=papel, sessoes=sessoes)
    @staticmethod
    def gid_por_sessao(name): return contexts.active_gid(name)
    @staticmethod
    def regras_path(gid):
        from app import pair
        return Path(call("roles.path", gid=gid, pair_directory=pair._pair_dir()))


markdown = Markdown()
roles = Roles()


@dataclass(frozen=True)
class Policy:
    conta: str
    provider: str
    apelido: str = ""
    modelos: tuple = ("*",)
    trocar: bool = True


@dataclass(frozen=True)
class InventoryAccount:
    conta: str
    provider: str
    apelido: str
    id_cota: str | None
    modelos: tuple = ()
    reduced: bool = False


@dataclass(frozen=True)
class CodexAccount:
    id: str
    home: Path
    is_default: bool


class CatalogUnavailable(RuntimeError):
    pass


from types import SimpleNamespace


class Policies:
    ContaPolitica = Policy
    ContaInventario = InventoryAccount
    config = SimpleNamespace(list_config_dirs=lambda: [])
    contas = SimpleNamespace(compartilhado=lambda: Path.home() / ".claude")
    cotas = SimpleNamespace(_providers_kimi=lambda: [])
    kimi_models = SimpleNamespace(read_catalog=lambda path=None: None)
    pi_catalog = SimpleNamespace(listar=lambda fresco=False: [])
    apelidos = SimpleNamespace(ler=lambda: {})
    codex_contas = SimpleNamespace(Account=CodexAccount, list_visible_accounts=lambda: [])
    codex_models = SimpleNamespace(listar=lambda fresco=False, codex_home=None: [], CodexIndisponivel=CatalogUnavailable)

    @staticmethod
    def caminho(): return Path.home() / ".hangar" / "orquestracao-contas.md"
    def ler(self, texto=None):
        rows = call("policy.read-file", path=self.caminho()) if texto is None else call("policy.ler", texto=texto)
        return [Policy(**(row | {"modelos": tuple(row["modelos"])})) for row in rows]
    def nome_conta_claude(self, path):
        return call("policy.name", base=self.contas.compartilhado(), path=path)
    def id_cota(self, provider, conta):
        return call("policy.quota", home=Path.home(), base=self.contas.compartilhado(), provider=provider, conta=conta)
    def permitido(self, provider, conta, modelo, esforco, politica=None):
        return call("policy.permitido", provider=provider, conta=conta, modelo=modelo, esforco=esforco,
                    politica=self.ler() if politica is None else politica, levels=self._levels(provider, modelo))
    def _levels(self, provider, modelo):
        catalog = self.kimi_models.read_catalog() if provider == "kimi" else None
        return next((m["efforts"] for m in (catalog or {}).get("models", []) if m["alias"] == modelo), None)
    def inventario(self, catalogo_claude=None):
        claude = []
        for account in self.config.list_config_dirs():
            models, reduced = catalogo_claude(Path(account.path)) if catalogo_claude else (None, True)
            claude.append({"account": {"path": account.path, "label": account.label}, "models": models, "reduced": reduced})
        try:
            pi = self.pi_catalog.listar()
        except Exception:
            pi = []
        codex = []
        for account in self.codex_contas.list_visible_accounts():
            try:
                models = self.codex_models.listar(codex_home=None if account.is_default else account.home)
            except Exception:
                models = []
            codex.append({"account": account, "models": models})
        rows = call("policy.inventory", base=self.contas.compartilhado(), aliases=self.apelidos.ler(),
                    claude=claude, kimi=self.kimi_models.read_catalog() or {},
                    kimi_names=[name for name, _key, _url in self.cotas._providers_kimi()], pi=pi, codex=codex)
        return [InventoryAccount(**(row | {"modelos": tuple(row["modelos"])})) for row in rows]
    def gravar_conta(self, conta, mtime_lido=None):
        return call("policy.write", path=self.caminho(), policy=conta, conta=conta.conta,
                    inventory=self.inventario(), mtime=mtime_lido)
    def desligar(self, conta, mtime_lido=None):
        return call("policy.write", path=self.caminho(), policy=None, conta=conta,
                    inventory=self.inventario(), mtime=mtime_lido)
    def migrar(self): return call("policy.migrate", home=Path.home(), base=self.contas.compartilhado())


policies = Policies()


class Identity:
    IdentityUnavailable = ContractError
    @staticmethod
    def identity(name, mux_bin="tmux"):
        return call("command", operation="identity", arguments={"home": str(Path.home()), "name":name,"mux":mux_bin})


identity = Identity()


class Timeline:
    from datetime import datetime,timezone
    orq_conductor=SimpleNamespace(_when=lambda value: __import__("datetime").datetime.fromtimestamp(call("runs.when",value=value),__import__("datetime").timezone.utc))
    @staticmethod
    def timeline_path(directory):
        return Path(call("timeline.path", directory=directory))
    @staticmethod
    def run_files(directory): return Path(directory)
    @staticmethod
    def entry(obj, run=None):
        return call("timeline.entry", obj=obj, **({"directory":run} if run is not None else {}))
    @staticmethod
    def panel(directory, live=None):
        return call("panel.build", directory=directory, home=Path.home())
    @staticmethod
    def _timing_bounds(evs): return call("panel._timing_bounds", evs=evs)
    @staticmethod
    def _with_timing(out, aux, now): return call("panel._with_timing", out=out,aux=aux,now=now.isoformat())
    @staticmethod
    def _rows_until(provider,path,until):
        result=call("consumption.rows",provider=provider,path=path,until=until.isoformat())
        return [SimpleNamespace(**r) for r in result] if result is not None else None
    @staticmethod
    def _rows_for(provider,path):
        result=call("consumption.rows",provider=provider,path=path)
        return [SimpleNamespace(**r) for r in result] if result is not None else None
    @staticmethod
    def _task_rows(evs, plan, integrated): return call("panel._task_rows", evs=evs,plan=plan,integrated=list(integrated))


timeline = Timeline()


class Adapter:
    @staticmethod
    def parse_obj(obj, run=None):
        return [SimpleNamespace(**row) for row in call("timeline.parse", obj=obj,directory=run)]
    def parse_line(self,line):
        try: obj=json.loads(line)
        except ValueError:return []
        return self.parse_obj(obj)
    def line_parser(self, run):
        def parse(line):
            try: obj=json.loads(line)
            except ValueError:return []
            return self.parse_obj(obj,run)
        return parse


adapter = Adapter()


@dataclass(frozen=True)
class Context:
    gid: str
    grouped: bool
    session_prefix: str
    path: Path


class Contexts:
    PromotionConflict = ContractError
    IdentityUnavailable = ContractError
    @staticmethod
    def identity(name): return identity.identity(name)
    @staticmethod
    def active_gid(name): return call("command",operation="active-gid",arguments={"home":str(Path.home()),"name":name})
    @staticmethod
    def _args():
        from app import pair
        return {"home":Path.home(),"pair_directory":pair._pair_dir()}
    def resolve(self,name):
        row=call("context.resolve",**self._args(),name=name)
        return Context(row["gid"],row["grouped"],row["prefix"],Path(row["path"]))
    def write(self,context,text,mtime):
        return call("context.write",**self._args(),context=context,texto=text,mtime=mtime)
    def promote(self,name,gid,mtime=None):
        return call("context.promote",**self._args(),name=name,gid=gid,mtime=mtime)


contexts=Contexts()


class Summary:
    @staticmethod
    def listar_execucoes(root):
        return [SimpleNamespace(**(row | {"tasks":[SimpleNamespace(**task) for task in row["tasks"]]})) for row in call("runs.list",directory=root)]
    @staticmethod
    def detalhe(root,name):
        row=call("runs.detail",directory=root,name=name)
        return SimpleNamespace(**(row|{"tasks":[SimpleNamespace(**task) for task in row["tasks"]]})) if row else None
    @staticmethod
    def fichas(rows):
        return call("runs.cards",runs=[vars(row)|{"tasks":[vars(task) for task in row.tasks]} for row in rows])
    @staticmethod
    def raiz_padrao(): return Path(call("runs.root",home=Path.home()))


summary=Summary()


class Runs:
    @staticmethod
    def root(): return Path(call("runs.root", home=Path.home()))
    @staticmethod
    def group_phase(gid): return call("runs.phase",home=Path.home(),gid=gid)


runs=Runs()
