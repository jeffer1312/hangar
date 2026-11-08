#!/usr/bin/env python3
"""Religa Correção Automática e Texto Preditivo pela tela de Ajustes do iPhone, via WebDriverAgent.

O WebDriverAgent desliga as duas ao subir, e o ajuste dele (`appium/settings`) responde "ligado"
sem mudar nada no iOS: só o toque na chave pega. Rótulos em pt-BR, como o aparelho está.
"""
import json
import sys
import time
import urllib.error
import urllib.request

WDA = "http://127.0.0.1:8100"
SWITCHES = ("Correção Automática", "Texto Preditivo")


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
    for attempt in range(5):
        try:
            s = call("POST", "/session", {"capabilities": {"alwaysMatch": {}}})["sessionId"]
            break
        except OSError:
            if attempt == 4:
                raise
            time.sleep(3)
    # Fechar antes de abrir: os Ajustes reabrem na última tela, e o caminho parte da raiz. A sessão
    # nova presa ao app é a que enxerga os elementos dele.
    call("POST", f"/session/{s}/wda/apps/terminate", {"bundleId": "com.apple.Preferences"})
    s = call("POST", "/session", {"capabilities": {"alwaysMatch": {"bundleId": "com.apple.Preferences"}}})["sessionId"]

    def find(pred):
        try:
            v = call("POST", f"/session/{s}/element", {"using": "predicate string", "value": pred})["value"]
        except urllib.error.HTTPError as e:
            if e.code == 404:  # é assim que o WebDriverAgent diz "não está na tela"
                return None
            raise
        return v.get("ELEMENT") if isinstance(v, dict) else None

    def scroll_to(e):
        try:
            call("POST", f"/session/{s}/wda/element/{e}/scrollTo", {})
        except urllib.error.HTTPError as err:
            if err.code != 404:  # item já visível às vezes responde 404; o toque segue
                raise

    def open_item(label):
        e = None
        attempts = 10  # a tela ainda pode estar abrindo
        can_swipe = True
        for attempt in range(attempts):
            e = find(f"label == '{label}' AND type != 'XCUIElementTypeNavigationBar'")
            if e or attempt == attempts - 1:
                break
            if attempt >= 2 and can_swipe:  # lista longa: o item fora da tela só entra na árvore depois de rolar
                try:
                    size = call("GET", f"/session/{s}/window/size")["value"]
                    x = size["width"] / 2
                    call("POST", f"/session/{s}/wda/dragfromtoforduration",
                         {"fromX": x, "fromY": size["height"] * 0.75, "toX": x, "toY": size["height"] * 0.35, "duration": 0.3})
                except (urllib.error.URLError, KeyError, TypeError) as err:  # sem rolar, a busca segue e o "não achei" sai no fim
                    print(f"aviso: a rolagem dos Ajustes falhou ({err})", file=sys.stderr)
                    can_swipe = False
            time.sleep(1)
        if not e:
            raise SystemExit(f"aviso: não achei '{label}' nos Ajustes; religue à mão em Ajustes > Geral > Teclado")
        scroll_to(e)
        call("POST", f"/session/{s}/element/{e}/click", {})
        time.sleep(1.5)

    def switch(label):
        return find(f"type == 'XCUIElementTypeSwitch' AND label == '{label}'")

    def is_on(label):
        # Busca de novo a cada leitura: ao virar a chave a tela redesenha e a referência antiga dá 404.
        e = switch(label)
        return e is not None and str(call("GET", f"/session/{s}/element/{e}/attribute/value")["value"]) == "1"

    failed = []
    try:
        open_item("Geral")
        open_item("Teclado")
        for label in SWITCHES:
            e = switch(label)
            if not e:
                failed.append(label)
                continue
            if not is_on(label):
                scroll_to(e)
                e = switch(label)  # a rolagem redesenha a tela
                if not e:
                    failed.append(label)
                    continue
                r = call("GET", f"/session/{s}/element/{e}/rect")["value"]
                # O clique no elemento cai no meio da linha e não vira a chave; o toque tem de ser nela.
                call("POST", f"/session/{s}/wda/tap", {"x": r["x"] + r["width"] - 30, "y": r["y"] + r["height"] / 2})
                time.sleep(1)
            if not is_on(label):
                failed.append(label)
    finally:
        # Sai dos Ajustes mesmo na falha: a próxima automação espera a tela inicial.
        call("POST", "/wda/homescreen")
    if failed:
        raise SystemExit("aviso: não religou " + ", ".join(failed) + "; religue à mão em Ajustes > Geral > Teclado")


if __name__ == "__main__":
    try:
        main()
    except OSError as e:
        sys.exit(f"aviso: WebDriverAgent não respondeu ao religar o teclado ({e})")
