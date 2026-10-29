//! A parada aguarda conexões que ainda podem devolver um layout registrado.
use std::sync::{Arc, Mutex};
use tokio::sync::{Notify, watch};

pub(super) struct Connections {
    state: Mutex<(bool, usize)>,
    stop: watch::Sender<bool>,
    ended: Notify,
}
impl Default for Connections {
    fn default() -> Self {
        Self { state: Mutex::new((false, 0)), stop: watch::channel(false).0, ended: Notify::new() }
    }
}
impl Connections {
    pub(super) fn enter(self: &Arc<Self>) -> Option<Connection> {
        let mut state = self.state.lock().unwrap();
        if state.0 { return None; }
        state.1 += 1;
        Some(Connection { owner: self.clone(), stop: self.stop.subscribe() })
    }
    pub(super) fn stop(&self) {
        self.state.lock().unwrap().0 = true;
        self.stop.send_replace(true);
    }
    pub(super) async fn shutdown(&self) {
        self.stop();
        loop {
            let ended = self.ended.notified();
            tokio::pin!(ended);
            ended.as_mut().enable();
            if self.state.lock().unwrap().1 == 0 { return; }
            ended.await;
        }
    }
}
pub(super) struct Connection {
    owner: Arc<Connections>,
    stop: watch::Receiver<bool>,
}
impl Connection {
    pub(super) fn is_stopping(&self) -> bool { *self.stop.borrow() }
    pub(super) async fn stopped(&mut self) {
        let _ = self.stop.wait_for(|stop| *stop).await;
    }
}
impl Drop for Connection {
    fn drop(&mut self) {
        self.owner.state.lock().unwrap().1 -= 1;
        self.owner.ended.notify_waiters();
    }
}
