import json
import os
import tempfile
import threading
from pathlib import Path
from typing import Any

from app import atomico
from app.config import _backend_config_base, settings

# Configuração editável em RUNTIME.
#
# Até aqui as ~30 settings vinham só de env/.env: mudar a chave da Groq ou a retenção de anexos
# exigia editar arquivo no servidor e reiniciar o serviço — do celular, impossível. Esta camada é um
# JSON que fica POR CIMA do env: quem lê usa `get(campo)`, que devolve o override quando existe e o
# valor do env quando não.
#
# O que NÃO entra aqui, de propósito: porta, IP de bind, token de auth, chaves VAPID e segredos de
# sync/deploy. A ativação do sync é editável, os segredos continuam no env. Os demais ou exigem
# reiniciar o processo, ou dariam ao celular o poder de mudar
# a própria fechadura. Essas continuam só no env — a tela mostra o valor em leitura e diz qual
# variável mexer.
EDITAVEIS: dict[str, type] = {
    "sync": bool,                 # ativa esta máquina como principal sem reiniciar
    "groq_api_key": str,          # transcrição de áudio e de vídeo
    "transcription_base_url": str,  # base OpenAI-compatible; vazio = serviço padrão
    "transcription_model": str,     # modelo de áudio; vazio = whisper-large-v3-turbo
    "upload_retention_days": int,  # dias que um anexo sobrevive
    "notify_finished": bool,
    "notify_dead": bool,
    "finish_min_seconds": int,
    "stall_seconds": int,
    "automations": bool,           # kill-switch das automações desatendidas
    "traduzir_pensamento": bool,   # resumo em pt-BR do raciocínio (desligado = texto original)
    "codex_sync": bool,            # reconciliação automática do Codex (por cima do automations)
    "codex_voice_beta": bool,      # conversa realtime do Codex (experimental, opt-in)
    "codex_memory_import": bool,   # leva as memórias do Claude pro Codex (opt-in: é o único
                                   # item da reconciliação que gasta cota, na consolidação)
    "claude_statusline_update": bool,  # permite ao instalador atualizar a barra do Claude Code
    "claude_function_hooks": bool,     # portão de plugin de function hook (acesso antecipado)
    "editor": str,
    "elevenlabs_api_key": str,     # sintese de voz (ouvir a selecao)
    "elevenlabs_voice_id": str,    # id da voz escolhida na conta
    "tts_local_cmd": str,          # comando externo opcional: texto no stdin, WAV no stdout
    "tts_max_chars": int,          # acima disso o app pede confirmacao antes de sintetizar
    # Ajustes de naturalidade da ElevenLabs (voice_settings), vindo de deslizantes na tela — o valor
    # e SEMPRE real (o slider nasce no padrao da ElevenLabs, nunca "vazio"). _coagir so tem tipo int
    # pra numero — sem float em EDITAVEIS — entao guardam o valor*100: tts_stability=50 -> 0.5 na
    # requisicao. Campo AUSENTE (usuario nunca tocou o slider) e campo IGUAL ao padrao da ElevenLabs
    # se comportam igual: tts.py:_ajustes_efetivos so manda a chave quando o valor FOGE do padrao.
    # tts_speed guarda 70-120 (velocidade 0.7x-1.2x); os outros tres guardam 0-100.
    "tts_stability": int,          # 0-100 = stability 0..1 (padrao ElevenLabs: 50 = 0.5)
    "tts_similarity_boost": int,   # 0-100 = similarity_boost 0..1 (padrao ElevenLabs: 75 = 0.75)
    "tts_style": int,              # 0-100 = style 0..1 (padrao ElevenLabs: 0)
    "tts_speed": int,              # 70-120 = speed 0.7..1.2 (padrao ElevenLabs: 100 = 1.0x)
    "llm_base_url": str,   # endpoint compativel com OpenAI (vazio = Groq)
    "llm_api_key": str,    # chave do provedor (so usada quando ha base_url proprio)
    "llm_model": str,      # nome do modelo (vazio = o padrao)
    # reasoning_effort mandado ao provedor. Vazio = campo AUSENTE do payload (o de sempre) — nem
    # todo provedor conhece a chave, e mandar pra quem nao conhece e 400. Serve pra DESLIGAR o
    # raciocinio ("none") num modelo que raciocina: a limpeza do ditado tem timeout de 8s e um
    # modelo pensando estoura isso. Ver narrar._esforco_raciocinio.
    "llm_reasoning_effort": str,
    # Provedor SO do estilo "briefing" do ditado. Vazio = usa o mesmo do resto. Existe porque
    # limpar/prosa querem rapidez (a pessoa espera o texto no campo) e o briefing quer o modelo que
    # estrutura melhor, que costuma ser mais lento. Ver narrar._provedor(perfil="briefing").
    "llm_briefing_base_url": str,
    "llm_briefing_api_key": str,
    "llm_briefing_model": str,
    # Palavras que a Whisper tem que grafar direito (nome de projeto, de sessao, jargao do seu
    # dia). Somadas a transcribe.VOCAB_BASE. Ver transcribe.vocabulario.
    "ditado_vocabulario": str,
    # Quanto o ditado pode mexer no que voce falou: "limpar" | "prosa" | "briefing".
    # Ver narrar.ESTILOS_DITADO — cada um e um prompt E um conjunto de travas diferente.
    "ditado_estilo": str,
    # Origens EXTRAS que podem abrir o terminal, no mesmo formato "a,b" do CP_TERM_ORIGINS.
    # SOMA com o env, ao contrario do scan_roots logo abaixo, que sobrescreve: aqui a lista e o
    # perimetro de quem PODE abrir o terminal, e um override por inteiro feito do celular tiraria
    # do ar a origem que o dono declarou no .env — inclusive a que ele esta usando pra editar.
    "term_origins": str,
    # Jev (typesafe.ai), que decide a navegacao do `hangar-preview objetivo`. A chave fica aqui e
    # nao no ambiente de quem sobe o servidor: a sessao so a recebe se tiver sido aberta com o
    # recurso ligado, e trocar de chave nao pede reinicio.
    "jev_api_key": str,
    # Como a sessao NOVA nasce quando ninguem disse nada. Mora no servidor, e nao no localStorage
    # da folha, porque os outros dois caminhos de criacao (hangar-send, MCP new_session) nao leem
    # navegador — so aqui a escolha vale nos tres. Quem pede explicito (`--jev`, `jev=true`)
    # continua vencendo naquela sessao, sem mexer neste padrao.
    "jev_padrao": bool,
    # LLM pequeno que escreve o valor de um campo que o chamador nao cobriu — OPCIONAL, e a mesma
    # ordem de precedencia que o CLI ja usa: base_url + api_key + modelo (endpoint compativel com
    # a OpenAI), senao cmd, senao o padrao do proprio CLI.
    "jev_texto_base_url": str,
    "jev_texto_api_key": str,
    "jev_texto_modelo": str,
    "jev_texto_cmd": str,
    # Raizes do seletor de pasta (fs-scanner), no MESMO formato "a,b" do CP_SCAN_ROOTS.
    # Override vale por inteiro (nao soma com o env); vazio = volta ao env. Ver
    # config.resolve_scan_roots, que le daqui primeiro.
    "scan_roots": str,
    # Fileira de atalhos do painel de sessao. JSON numa string porque _coagir so conhece escalar;
    # vazio = conjunto nativo. Shape validado em _validate_shortcuts — config quebrada aqui viraria
    # fileira sumida sem erro em lugar nenhum.
    "shortcuts": str,
}

# Campos que NUNCA voltam inteiros pro cliente: o app devolve mascarado (gsk_••••1234) pra você
# conferir QUAL chave está lá sem poder copiá-la de volta.
# Explícito mesmo quando o nome já casaria com `_PALAVRAS_DE_SEGREDO`: depender do acaso do nome
# quebra calado no dia em que alguém renomeia o campo.
SEGREDOS = {"groq_api_key", "elevenlabs_api_key", "llm_api_key", "llm_briefing_api_key",
            "jev_api_key", "jev_texto_api_key"}

# Campo que a tela edita mas que NÃO mora neste arquivo: a verdade é o `settings.json` do Claude
# Code, porque quem o lê é o `claude` na largada da sessão. Guardar uma cópia aqui daria dois
# valores divergindo assim que alguém editasse aquele arquivo à mão. Ver app/pensamento.py.
EXTERNOS: dict[str, type] = {
    "mostrar_pensamento": bool,   # settings.json["showThinkingSummaries"]
}

_ARQUIVO = "runtime-config.json"

# Serializa o read-modify-write: dois PATCH ao mesmo tempo liam o mesmo estado e o ultimo a
# gravar apagava a mudanca do outro, calado.
_LOCK = threading.Lock()


def _caminho() -> Path:
    return Path(_backend_config_base()) / _ARQUIVO


def _carregar() -> dict[str, Any]:
    try:
        with open(_caminho(), encoding="utf-8") as fh:
            d = json.load(fh)
        return d if isinstance(d, dict) else {}
    except (OSError, UnicodeDecodeError, json.JSONDecodeError):
        # Arquivo ausente/corrompido não pode derrubar o backend: sem override, vale o env.
        #
        # UnicodeDecodeError é a segunda forma de "corrompido", e ela NÃO é json.JSONDecodeError:
        # o arquivo é lido como utf-8, então basta um byte que não seja utf-8 (editado à mão num
        # editor cp1252, ou uma escrita cortada no meio de um caractere multibyte) pra exceção
        # atravessar este except. E ela sobe num caminho quente — `get()` é quem responde
        # `automations_enabled()` e `resolve_scan_roots()`. A promessa da linha acima já era essa;
        # o que faltava era o except cumpri-la.
        return {}


def get(campo: str) -> Any:
    """Valor efetivo: override do arquivo, se houver; senão o do env."""
    if campo in EDITAVEIS:
        d = _carregar()
        if campo in d:
            return d[campo]
    return getattr(settings, campo, None)


# Campo daqui -> variável que o `hangar-preview` já lê hoje.
_JEV_TEXTO = (
    ("jev_texto_base_url", "JEV_TEXTO_BASE_URL"),
    ("jev_texto_api_key", "JEV_TEXTO_API_KEY"),
    ("jev_texto_modelo", "JEV_TEXTO_MODELO"),
    ("jev_texto_cmd", "JEV_TEXTO_CMD"),
)
# Marcador do estado do recurso NA SESSÃO. Vai sempre, ligado ou desligado: sem ele o
# `hangar-preview objetivo` não separa "desligado nesta sessão" de "nunca configurado", e as duas
# pedem frases diferentes.
MARCA_JEV = "HANGAR_JEV"


def env_function_hooks() -> dict[str, str]:
    """Portão de plugin de function hook, para sessão CLAUDE. Ao contrário do Jev, não há marcador
    de desligado: a ausência da variável É o desligado, e é ela que o Claude Code lê.

    Lê a configuração no momento em que a sessão SOBE — inclusive no relançamento. Não é escolha de
    sessão que se preserva (isso é o `jev`), é configuração do servidor: sessão relançada reflete o
    que está ligado agora."""
    return {"CLAUDE_CODE_ENABLE_FUNCTION_HOOKS": "1"} if get("claude_function_hooks") else {}


def env_jev(ligado: bool) -> dict[str, str]:
    """Ambiente do Jev pra uma sessão. Desligado, só o marcador — a chave não entra no processo.

    A chave é segredo e, ligada, fica legível por quem já roda dentro daquela sessão. O ganho sobre
    deixá-la no `settings.json` não é sigilo: é ser por sessão e por escolha, em vez de global e em
    claro num arquivo que todas as contas compartilham."""
    env = {MARCA_JEV: "on" if ligado else "off"}
    if not ligado:
        return env
    chave = str(get("jev_api_key") or "").strip()
    if chave:
        env["TYPESAFE_API_KEY"] = chave
    for campo, var in _JEV_TEXTO:
        valor = str(get(campo) or "").strip()
        if valor:
            env[var] = valor
    return env


def override(campo: str) -> tuple[bool, Any]:
    """Presença e valor cru do override, para rollback preservar a origem da configuração."""
    d = _carregar()
    return campo in d, d.get(campo)


def mascarar(valor: str) -> str:
    """Segredo em forma conferível, não copiável: mostra só o começo e o fim."""
    if not valor:
        return ""
    if len(valor) <= 8:
        return "•" * len(valor)
    return f"{valor[:4]}{'•' * 8}{valor[-4:]}"


# As acoes internas que a fileira conhece (os botoes nativos de hoje). Item com action fora
# daqui seria um botao morto na tela — recusa na gravacao, apontando o item.
_SHORTCUT_INTERNAL_ACTIONS = {"terminal", "modo", "navegador", "anexos", "rodar"}


def _validate_shortcuts(text: str) -> None:
    """Recusa na gravacao o que o front nao conseguiria renderizar. O resolve do front e
    tolerante (config invalida cai no conjunto nativo), entao sem esta trava um typo salvo
    pela API viraria "minha fileira voltou ao padrao" sem nenhum erro visivel."""
    try:
        items = json.loads(text)
    except ValueError:
        raise ValueError("shortcuts: JSON invalido") from None
    if not isinstance(items, list):
        raise ValueError("shortcuts: esperado uma lista de atalhos")
    for i, item in enumerate(items, start=1):
        if not isinstance(item, dict):
            raise ValueError(f"shortcuts: item {i} nao e um objeto")
        if not isinstance(item.get("id"), str) or not item["id"].strip():
            raise ValueError(f"shortcuts: item {i} sem id")
        kind = item.get("type")
        if kind == "internal":
            if item.get("action") not in _SHORTCUT_INTERNAL_ACTIONS:
                raise ValueError(
                    f"shortcuts: item {i} tem action desconhecida "
                    f"(use uma de: {', '.join(sorted(_SHORTCUT_INTERNAL_ACTIONS))})"
                )
        elif kind == "send_text":
            if not isinstance(item.get("text"), str) or not item["text"].strip():
                raise ValueError(f"shortcuts: item {i} (send_text) sem texto a enviar")
        elif kind == "shell":
            if not isinstance(item.get("command"), str) or not item["command"].strip():
                raise ValueError(f"shortcuts: item {i} (shell) sem comando")
        else:
            raise ValueError(f"shortcuts: item {i} tem type desconhecido '{kind}'")
        if kind in ("send_text", "shell") and (
            not isinstance(item.get("label"), str) or not item["label"].strip()
        ):
            raise ValueError(f"shortcuts: item {i} sem rotulo")


def _coagir(campo: str, valor: Any) -> Any:
    """Converte o que veio do JSON pro tipo do campo. Levanta ValueError no que não dá."""
    tipo = EDITAVEIS[campo]
    if tipo is bool:
        if isinstance(valor, bool):
            return valor
        raise ValueError(f"{campo}: esperado true/false")
    if tipo is int:
        if isinstance(valor, bool) or not isinstance(valor, (int, float, str)):
            raise ValueError(f"{campo}: esperado número")
        try:
            n = int(valor)
        except (TypeError, ValueError):
            raise ValueError(f"{campo}: esperado número") from None
        if n < 0:
            raise ValueError(f"{campo}: não pode ser negativo")
        return n
    if not isinstance(valor, str):
        raise ValueError(f"{campo}: esperado texto")
    texto = valor.strip()
    if campo == "editor" and texto:
        # O editor vira argv[0] de um subprocess. Enquanto vinha so do .env, quem escolhia era o dono
        # da maquina; agora o celular escreve. Nome NU (sem barra, sem ..) mantem a escolha livre
        # (code, nvim, subl) e impede apontar pra um binario solto tipo /tmp/qualquer.sh.
        if "/" in texto or "\\" in texto or texto.startswith("-") or ".." in texto:
            raise ValueError("editor: use o nome do binario (ex: code), sem caminho")
    if campo == "ditado_estilo" and texto:
        # Import local pelo mesmo motivo do vocabulario abaixo (transcribe/narrar importam este
        # modulo). Recusar aqui e o que impede um estilo inexistente virar "nenhuma limpeza,
        # calado": narrar cai no padrao quando nao reconhece o valor, entao sem esta trava um typo
        # na config faria o ditado piorar sem nada na tela dizendo por que.
        from app.narrar import ESTILOS_DITADO
        if texto not in ESTILOS_DITADO:
            raise ValueError(
                f"ditado_estilo: '{texto}' nao existe. Use um de: {', '.join(ESTILOS_DITADO)}."
            )
    if campo == "scan_roots" and texto:
        # resolve_scan_roots descarta calado entrada que nao e diretorio (um typo no env nunca
        # alarga o perimetro). Vindo da TELA, o descarte calado vira "adicionei a pasta, salvou,
        # e o chip nunca apareceu" — recusa aqui, nomeando a entrada ruim.
        for entrada in texto.split(","):
            entrada = entrada.strip()
            if entrada and not Path(os.path.realpath(os.path.expanduser(entrada))).is_dir():
                raise ValueError(f"scan_roots: '{entrada}' nao e um diretorio nesta maquina")
    if campo == "ditado_vocabulario" and texto:
        # Import LOCAL: transcribe importa este modulo, entao um import no topo fecharia o ciclo —
        # mesmo motivo (e mesma solucao) de config.automations_enabled.
        #
        # O teto vive AQUI, e nao so no corte de transcribe.vocabulario, porque este e o unico
        # ponto da corrente que consegue falar com a pessoa. Cortando so na leitura, ela cadastra
        # 40 termos, a tela diz "salvo", e os ultimos simplesmente nunca chegam na Whisper: os
        # nomes que ela configurou pra parar de sair errado continuam saindo errado, sem nada em
        # lugar nenhum explicando por que. Recusar na gravacao transforma isso num erro visivel no
        # segundo em que ela aperta salvar.
        from app.transcribe import VOCAB_USUARIO_MAX
        if len(texto) > VOCAB_USUARIO_MAX:
            raise ValueError(
                f"ditado_vocabulario: {len(texto)} caracteres, o maximo e {VOCAB_USUARIO_MAX} "
                "(a Whisper ignora o resto). Tire os termos que voce menos erra."
            )
    if campo == "term_origins" and texto:
        # Entrada aqui vira permissao de abrir terminal na maquina. O que a checagem compara e o
        # netloc, entao aceitar texto solto ("pocket") deixaria a pessoa salvar uma linha que nunca
        # casa e concluir que o app ignorou o que ela configurou — o mesmo defeito que o
        # scan_roots resolveu recusando na gravacao.
        from urllib.parse import urlparse
        for entrada in texto.split(","):
            entrada = entrada.strip()
            if not entrada:
                continue
            if not (entrada.startswith("http://") or entrada.startswith("https://")):
                raise ValueError(f"term_origins: '{entrada}' precisa comecar com http:// ou https://")
            if not urlparse(entrada).netloc:
                raise ValueError(f"term_origins: '{entrada}' nao tem endereco (ex: https://app.exemplo.com)")
    if campo == "shortcuts" and texto:
        _validate_shortcuts(texto)
    if campo in ("transcription_base_url", "llm_base_url", "llm_briefing_base_url") and texto and not (texto.startswith("http://") or texto.startswith("https://")):
        # Mesmo argumento do editor: antes so o dono da maquina escolhia o endpoint (env), agora o
        # celular escreve. Aceita vazio (volta ao padrao) ou uma URL http(s) de verdade.
        raise ValueError(f"{campo}: use vazio ou uma URL http(s)://")
    return texto


def aplicar(mudancas: dict[str, Any], *, remover: set[str] | None = None) -> dict[str, Any]:
    """Grava os overrides. Ignora campo desconhecido (não deixa o cliente inventar setting).

    Escrita atômica (tmp + replace): um corte de energia no meio não deixa um JSON pela metade,
    que na próxima leitura viraria "sem override nenhum" — perder a configuração inteira calado.
    """
    with _LOCK:
        return _aplicar_travado(mudancas, remover or set())


def _aplicar_travado(mudancas: dict[str, Any], remover: set[str]) -> dict[str, Any]:
    atual = _carregar()
    for campo in remover:
        if campo in EDITAVEIS:
            atual.pop(campo, None)
    # Campo EXTERNO grava em OUTRO arquivo (o settings.json do Claude), então ele fica pro fim: o
    # front manda o rascunho INTEIRO num POST só, e um campo inválido no meio levantava ValueError
    # DEPOIS de a chave já ter sido escrita lá. A tela mostrava o erro e mantinha o rascunho — ou
    # seja, a pessoa achava que não tinha ligado o resumo, e tinha.
    externos = {}
    for campo, valor in mudancas.items():
        if campo in EXTERNOS:
            externos[campo] = valor
            continue
        if campo not in EDITAVEIS:
            continue
        # Segredo devolvido MASCARADO tem que ser reconhecido e ignorado. A checagem antiga era
        # "a string é só bullets?" — mas a máscara real é mista (gsk_••••••••1234), então NUNCA
        # batia: encostar no campo sobrescrevia a chave verdadeira pelo texto mascarado, sem volta.
        # Compara com a máscara do valor ATUAL, que é exatamente o que o cliente recebeu.
        if campo in SEGREDOS and isinstance(valor, str):
            efetivo = atual.get(campo) if campo in atual else getattr(settings, campo, "")
            if valor.strip() in {mascarar(efetivo or ""), ""} and efetivo:
                continue
        atual[campo] = _coagir(campo, valor)
    # Tipo errado num campo externo também tem que barrar ANTES de qualquer escrita — validar aqui
    # e gravar depois deixa os dois arquivos combinando com o que a tela diz.
    for campo, valor in externos.items():
        if not isinstance(valor, EXTERNOS[campo]):
            raise ValueError(f"{campo}: esperado true/false")
    # Escreve o EXTERNO primeiro. Não há como comitar dois arquivos junto, então a ordem escolhe
    # qual falha deixa a máquina inteira. A falha realista aqui é o settings.json ilegível — e
    # nessa ordem ela para tudo antes de gravar qualquer coisa. A ordem contrária gravaria o
    # runtime-config e só então descobriria o problema, com a tela dizendo que nada foi salvo.
    for campo, valor in externos.items():
        _gravar_externo(campo, valor)
    destino = _caminho()
    destino.parent.mkdir(parents=True, exist_ok=True)
    fd, tmp = tempfile.mkstemp(dir=str(destino.parent), suffix=".tmp")
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as fh:
            json.dump(atual, fh, ensure_ascii=False, indent=2)
        atomico.substituir(tmp, destino)
        # O arquivo guarda segredo (chave da Groq): 0600 como o .env, pra não ficar legível por
        # outro usuário da máquina. Falha de chmod não desfaz a gravação — o valor já está lá.
        try:
            os.chmod(destino, 0o600)
        except OSError:
            pass
    except BaseException:
        Path(tmp).unlink(missing_ok=True)
        raise
    return atual


def _gravar_externo(campo: str, valor: Any) -> None:
    """Campo de EXTERNOS: valida como os outros e delega a quem é dono do arquivo.

    Erro de escrita SOBE (vira 400 na tela) em vez de virar log: o interruptor tem que dizer que
    não pegou, senão a pessoa acha que ligou o resumo e a próxima sessão nasce sem ele.
    """
    if not isinstance(valor, bool):
        raise ValueError(f"{campo}: esperado true/false")
    from app import pensamento
    try:
        pensamento.gravar(valor)
    except (OSError, RuntimeError) as e:
        raise ValueError(f"{campo}: {e}") from e


def estado() -> dict[str, Any]:
    """O que a tela mostra: valor efetivo de cada campo editável (segredo já mascarado) e se ele
    está vindo de um override ou do env."""
    overrides = _carregar()
    out: dict[str, Any] = {}
    for campo in EDITAVEIS:
        valor = get(campo)
        out[campo] = {
            "valor": mascarar(valor or "") if campo in SEGREDOS else valor,
            "definido": bool(valor) if campo in SEGREDOS else valor is not None,
            "origem": "app" if campo in overrides else "env",
        }
    from app import pensamento
    out["mostrar_pensamento"] = {
        "valor": pensamento.ler(),
        "definido": True,
        # "app" = a chave está escrita no settings.json (a tela marca a linha como editada); sem
        # ela o Claude Code trata como desligado, que é o "padrão" desta máquina.
        "origem": "app" if pensamento.definido() else "env",
    }
    return out
