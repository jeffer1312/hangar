#!/usr/bin/env python3
"""Religa Correção Automática e Texto Preditivo pela tela de Ajustes do iPhone, via WebDriverAgent.

O WebDriverAgent desliga as duas ao subir, e o ajuste dele (`appium/settings`) responde "ligado"
sem mudar nada no iOS 26 — só o toque na chave pega. Rótulos em pt-BR, como o aparelho está.
"""
import json
import sys
import time
import urllib.error
import urllib.request

WDA = "http://127.0.0.1:8100"
CHAVES = ("Correção Automática", "Texto Preditivo")


def call(method, path, body=None):
    req = urllib.request.Request(WDA + path, method=method,
                                 data=None if body is None else json.dumps(body).encode(),
                                 headers={"Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=30) as r:
            return json.load(r)
    except urllib.error.HTTPError as e:
        e.msg = f"{e.msg} em {method} {path}"
        raise


def main():
    # Logo depois do "ready" a primeira sessão às vezes cai sem resposta.
    for tentativa in range(5):
        try:
            s = call("POST", "/session", {"capabilities": {"alwaysMatch": {}}})["sessionId"]
            break
        except OSError:
            if tentativa == 4:
                raise
            time.sleep(3)
    # Fechar antes de abrir: os Ajustes reabrem na última tela, e o caminho parte da raiz. A sessão
    # nova presa ao app é a que enxerga os elementos dele.
    call("POST", f"/session/{s}/wda/apps/terminate", {"bundleId": "com.apple.Preferences"})
    s = call("POST", "/session", {"capabilities": {"alwaysMatch": {"bundleId": "com.apple.Preferences"}}})["sessionId"]

    def achar(pred):
        try:
            v = call("POST", f"/session/{s}/element", {"using": "predicate string", "value": pred})["value"]
        except urllib.error.HTTPError as e:
            if e.code == 404:  # é assim que o WebDriverAgent diz "não está na tela"
                return None
            raise
        return v.get("ELEMENT") if isinstance(v, dict) else None

    def rolar(e):
        try:
            call("POST", f"/session/{s}/wda/element/{e}/scrollTo", {})
        except urllib.error.HTTPError as err:
            if err.code != 404:  # item já visível às vezes responde 404; o toque segue
                raise

    def abrir(rotulo):
        e = None
        for _ in range(10):  # a tela ainda pode estar abrindo
            e = achar(f"label == '{rotulo}' AND type != 'XCUIElementTypeNavigationBar'")
            if e:
                break
            time.sleep(1)
        if not e:
            raise SystemExit(f"aviso: não achei '{rotulo}' nos Ajustes; religue à mão em Ajustes > Geral > Teclado")
        rolar(e)
        call("POST", f"/session/{s}/element/{e}/click", {})
        time.sleep(1.5)

    abrir("Geral")
    abrir("Teclado")
    falhou = []
    def chave(rotulo):
        return achar(f"type == 'XCUIElementTypeSwitch' AND label == '{rotulo}'")

    def ligada(rotulo):
        # Busca de novo a cada leitura: ao virar a chave a tela redesenha e a referência antiga dá 404.
        e = chave(rotulo)
        return e is not None and str(call("GET", f"/session/{s}/element/{e}/attribute/value")["value"]) == "1"

    for rotulo in CHAVES:
        e = chave(rotulo)
        if not e:
            falhou.append(rotulo)
            continue
        if not ligada(rotulo):
            rolar(e)
            r = call("GET", f"/session/{s}/element/{chave(rotulo)}/rect")["value"]
            # O clique no elemento cai no meio da linha e não vira a chave; o toque tem de ser nela.
            call("POST", f"/session/{s}/wda/tap", {"x": r["x"] + r["width"] - 30, "y": r["y"] + r["height"] / 2})
            time.sleep(1)
        if not ligada(rotulo):
            falhou.append(rotulo)
    call("POST", "/wda/homescreen")
    if falhou:
        raise SystemExit("aviso: não religou " + ", ".join(falhou) + "; religue à mão em Ajustes > Geral > Teclado")


if __name__ == "__main__":
    try:
        main()
    except OSError as e:
        sys.exit(f"aviso: WebDriverAgent não respondeu ao religar o teclado ({e})")
