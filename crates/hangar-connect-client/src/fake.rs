//! Dublê do transporte: responde por uma função, grava os pedidos e tem um relógio que só anda no `sleep`.
use std::future::Future;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::error::Error;
use crate::transport::{Method, Request, Response, Transport};

type Handler = Box<dyn Fn(&Request) -> Result<Response, Error> + Send + Sync>;

pub(crate) struct Fake {
    handler: Handler,
    calls: Mutex<Vec<Request>>,
    start: Instant,
    elapsed: Mutex<Duration>,
}

impl Fake {
    pub(crate) fn new(handler: impl Fn(&Request) -> Result<Response, Error> + Send + Sync + 'static) -> Self {
        Self { handler: Box::new(handler), calls: Mutex::new(Vec::new()), start: Instant::now(), elapsed: Mutex::new(Duration::ZERO) }
    }

    pub(crate) fn calls(&self) -> Vec<Request> {
        self.calls.lock().unwrap().clone()
    }

    /// `MÉTODO host/caminho`, sem a consulta.
    pub(crate) fn routes(&self) -> Vec<String> {
        self.calls().iter().map(route).collect()
    }

    pub(crate) fn slept(&self) -> Duration {
        *self.elapsed.lock().unwrap()
    }
}

impl Transport for Fake {
    fn send(&self, request: Request) -> impl Future<Output = Result<Response, Error>> + Send {
        let answer = (self.handler)(&request);
        self.calls.lock().unwrap().push(request);
        std::future::ready(answer)
    }

    fn sleep(&self, duration: Duration) -> impl Future<Output = ()> + Send {
        *self.elapsed.lock().unwrap() += duration;
        std::future::ready(())
    }

    fn now(&self) -> Instant {
        self.start + *self.elapsed.lock().unwrap()
    }
}

pub(crate) fn json(status: u16, body: Value) -> Result<Response, Error> {
    Ok(Response { status, set_cookies: Vec::new(), body: body.to_string().into_bytes() })
}

pub(crate) fn body(request: &Request) -> Value {
    request.body.as_deref().map(|body| serde_json::from_slice(body).unwrap()).unwrap_or(Value::Null)
}

pub(crate) fn header<'a>(request: &'a Request, name: &str) -> Option<&'a str> {
    request.headers.iter().find(|(key, _)| key.eq_ignore_ascii_case(name)).map(|(_, value)| value.as_str())
}

pub(crate) fn host(request: &Request) -> String {
    url::Url::parse(&request.url).unwrap().host_str().unwrap().to_owned()
}

pub(crate) fn path(request: &Request) -> String {
    url::Url::parse(&request.url).unwrap().path().to_owned()
}

pub(crate) fn route(request: &Request) -> String {
    let method = match request.method {
        Method::Get => "GET",
        Method::Post => "POST",
        Method::Put => "PUT",
        Method::Delete => "DELETE",
    };
    format!("{method} {}{}", host(request), path(request))
}
