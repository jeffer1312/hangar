//! CDP do WebView2 dentro do processo: sem porta de depuração, nada de fora se conecta.
//! Toda chamada e todo retorno acontecem na thread da interface (modelo de threads do WebView2).
use std::{cell::RefCell, future::Future};

use serde_json::Value;
use webview2_com::{
    CallDevToolsProtocolMethodCompletedHandler, CoTaskMemPWSTR, DevToolsProtocolEventReceivedEventHandler,
    Microsoft::Web::WebView2::Win32::{ICoreWebView2, ICoreWebView2DevToolsProtocolEventReceiver},
    take_pwstr,
};
use windows::core::PWSTR;

pub struct Cdp {
    view: ICoreWebView2,
    /// Guardados para o receptor viver tanto quanto a ponte.
    _receivers: RefCell<Vec<ICoreWebView2DevToolsProtocolEventReceiver>>,
}

impl Cdp {
    pub fn new(view: ICoreWebView2) -> Self { Self { view, _receivers: RefCell::default() } }

    pub fn call(&self, method: &str, params: Value) -> impl Future<Output = Result<Value, String>> + use<> {
        let (tx, rx) = futures::channel::oneshot::channel();
        let handler = CallDevToolsProtocolMethodCompletedHandler::create(Box::new(move |result, json: String| {
            let _ = tx.send(match result {
                Ok(()) => serde_json::from_str(&json).map_err(|e| e.to_string()),
                // O WebView2 devolve o erro do CDP no próprio JSON ({"code":..,"message":..}).
                Err(e) => Err(serde_json::from_str::<Value>(&json).ok()
                    .and_then(|v| v["message"].as_str().map(str::to_owned))
                    .unwrap_or_else(|| e.message().to_string())),
            });
            Ok(())
        }));
        let method = CoTaskMemPWSTR::from(method);
        let params = CoTaskMemPWSTR::from(params.to_string().as_str());
        // SAFETY: chamada na thread da interface, dona do WebView2; as strings vivem até o retorno.
        let started = unsafe { self.view.CallDevToolsProtocolMethod(*method.as_ref().as_pcwstr(), *params.as_ref().as_pcwstr(), &handler) };
        async move {
            started.map_err(|e| e.message().to_string())?;
            rx.await.map_err(|_| "cdp: a resposta nao voltou".to_string())?
        }
    }

    /// O WebView2 fechou ou o processo do navegador caiu: depois disso toda chamada à vista falha, até a leitura do pid.
    /// Com a vista de pé, a leitura passa, e a falha de uma chamada CDP fica sendo da página.
    pub fn closed(&self) -> bool {
        let mut pid = 0u32;
        // SAFETY: leitura de propriedade na thread da interface, dona do WebView2.
        unsafe { self.view.BrowserProcessId(&mut pid) }.is_err()
    }

    /// Chamada sem esperar resposta; a falha vai para o log, porque o motor não tem executor para aguardar.
    pub fn fire(&self, method: &str, params: Value) {
        let label = method.to_owned();
        let handler = CallDevToolsProtocolMethodCompletedHandler::create(Box::new(move |result, json: String| {
            if let Err(e) = result {
                let message = serde_json::from_str::<Value>(&json).ok()
                    .and_then(|v| v["message"].as_str().map(str::to_owned))
                    .unwrap_or_else(|| e.message().to_string());
                eprintln!("[nav] {label} falhou: {message}");
            }
            Ok(())
        }));
        let (name, args) = (CoTaskMemPWSTR::from(method), CoTaskMemPWSTR::from(params.to_string().as_str()));
        // SAFETY: mesma thread da interface; as strings vivem até o retorno.
        if let Err(e) = unsafe { self.view.CallDevToolsProtocolMethod(*name.as_ref().as_pcwstr(), *args.as_ref().as_pcwstr(), &handler) } {
            eprintln!("[nav] {method} nao saiu: {}", e.message());
        }
    }

    pub fn on(&self, event: &str, mut f: impl FnMut(Value) + 'static) -> Result<(), String> {
        let name = CoTaskMemPWSTR::from(event);
        // SAFETY: mesma thread da interface.
        let receiver = unsafe { self.view.GetDevToolsProtocolEventReceiver(*name.as_ref().as_pcwstr()) }.map_err(|e| e.message().to_string())?;
        let event = event.to_owned();
        let handler = DevToolsProtocolEventReceivedEventHandler::create(Box::new(move |_, args| {
            if let Some(args) = args {
                let mut json = PWSTR::null();
                // SAFETY: o WebView2 aloca a string; `take_pwstr` a libera.
                if let Err(e) = unsafe { args.ParameterObjectAsJson(&mut json) } {
                    eprintln!("[nav] evento {event} sem parametros: {}", e.message());
                    return Err(e);
                }
                match serde_json::from_str(&take_pwstr(json)) {
                    Ok(value) => f(value),
                    Err(e) => eprintln!("[nav] evento {event} com JSON invalido: {e}"),
                }
            }
            Ok(())
        }));
        let mut token = 0i64;
        // SAFETY: mesma thread da interface.
        unsafe { receiver.add_DevToolsProtocolEventReceived(&handler, &mut token) }.map_err(|e| e.message().to_string())?;
        self._receivers.borrow_mut().push(receiver);
        Ok(())
    }
}
