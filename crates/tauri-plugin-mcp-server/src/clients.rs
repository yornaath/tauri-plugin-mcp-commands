//! Connected-client tracking and the optional per-client approval prompt.
//!
//! Client identity is what the client reports (`clientInfo`), falling back to
//! its `User-Agent`. It is self-reported and only used for the approval UX and
//! status display; the bearer token is the security boundary.

use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Runtime};
use tokio::sync::watch;

pub const EVENT_CLIENT_CONNECTED: &str = "mcp://client-connected";
pub const EVENT_ACCESS_REQUESTED: &str = "mcp://access-requested";

/// What a request says about its client.
#[derive(Debug, Clone, Default)]
pub(crate) struct ClientIdent {
    pub(crate) name: Option<String>,
    pub(crate) version: Option<String>,
    pub(crate) user_agent: Option<String>,
}

/// A client that has talked to the server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct ConnectedClient {
    /// Stable key for this client during the app's run.
    pub key: String,
    pub name: Option<String>,
    pub version: Option<String>,
    pub user_agent: Option<String>,
    /// Unix milliseconds.
    // Unix ms and request ids stay far below 2^53, so a JS number holds them.
    #[cfg_attr(feature = "specta", specta(type = f64))]
    pub first_seen: u64,
    #[cfg_attr(feature = "specta", specta(type = f64))]
    pub last_seen: u64,
}

/// Payload of `mcp://access-requested`. Answer with `McpServer::respond_access`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct AccessRequest {
    #[cfg_attr(feature = "specta", specta(type = f64))]
    pub id: u64,
    pub client: ConnectedClient,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Decision {
    Allowed,
    Denied,
}

struct Pending {
    request: AccessRequest,
    tx: watch::Sender<Option<bool>>,
}

#[derive(Default)]
struct State {
    known: HashMap<String, (ConnectedClient, Decision)>,
    /// User-Agent → key of a client first identified by `clientInfo`, so its
    /// later requests that only carry the User-Agent map to the same client.
    aliases: HashMap<String, String>,
    pending: HashMap<String, Pending>,
    next_id: u64,
}

pub(crate) struct Clients {
    require_approval: bool,
    timeout: Duration,
    state: Mutex<State>,
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

impl Clients {
    pub(crate) fn new(require_approval: bool, timeout: Duration) -> Self {
        Self { require_approval, timeout, state: Mutex::new(State::default()) }
    }

    /// Lets a request through, asking the host app first when approval is on
    /// and the client is new. Concurrent requests from a pending client share
    /// one prompt.
    pub(crate) async fn admit<R: Runtime>(&self, app: &AppHandle<R>, ident: ClientIdent) -> Result<(), String> {
        let (key, mut rx) = {
            let mut st = self.state.lock().unwrap();
            let key = match (&ident.name, &ident.user_agent) {
                (Some(name), _) => format!("name:{name}"),
                (None, Some(ua)) => st.aliases.get(ua).cloned().unwrap_or_else(|| format!("ua:{ua}")),
                (None, None) => "unknown".to_owned(),
            };
            if let (Some(_), Some(ua)) = (&ident.name, &ident.user_agent) {
                st.aliases.entry(ua.clone()).or_insert_with(|| key.clone());
            }
            if let Some((client, decision)) = st.known.get_mut(&key) {
                client.last_seen = now_ms();
                return match decision {
                    Decision::Allowed => Ok(()),
                    Decision::Denied => Err(denied(client)),
                };
            }
            let now = now_ms();
            let client = ConnectedClient {
                key: key.clone(),
                name: ident.name.clone(),
                version: ident.version.clone(),
                user_agent: ident.user_agent.clone(),
                first_seen: now,
                last_seen: now,
            };
            if !self.require_approval {
                st.known.insert(key, (client.clone(), Decision::Allowed));
                drop(st);
                let _ = app.emit(EVENT_CLIENT_CONNECTED, &client);
                return Ok(());
            }
            let rx = match st.pending.get(&key) {
                Some(p) => p.tx.subscribe(),
                None => {
                    st.next_id += 1;
                    let request = AccessRequest { id: st.next_id, client };
                    let (tx, rx) = watch::channel(None);
                    st.pending.insert(key.clone(), Pending { request: request.clone(), tx });
                    drop(st);
                    let _ = app.emit(EVENT_ACCESS_REQUESTED, &request);
                    rx
                }
            };
            (key, rx)
        };

        if tokio::time::timeout(self.timeout, rx.wait_for(Option::is_some)).await.is_err() {
            // No answer in time: deny (a no-op if the host answered meanwhile).
            self.decide_key(app, &key, false);
        }
        let st = self.state.lock().unwrap();
        match st.known.get(&key) {
            Some((_, Decision::Allowed)) => Ok(()),
            Some((c, Decision::Denied)) => Err(denied(c)),
            // Reset while waiting (server stopped or token rotated).
            None => Err("the request was cancelled".into()),
        }
    }

    /// Answers a pending access request. Returns false if `id` is unknown.
    pub(crate) fn respond<R: Runtime>(&self, app: &AppHandle<R>, id: u64, allow: bool) -> bool {
        let key = {
            let st = self.state.lock().unwrap();
            st.pending.iter().find(|(_, p)| p.request.id == id).map(|(k, _)| k.clone())
        };
        match key {
            Some(key) => {
                self.decide_key(app, &key, allow);
                true
            }
            None => false,
        }
    }

    fn decide_key<R: Runtime>(&self, app: &AppHandle<R>, key: &str, allow: bool) {
        let mut st = self.state.lock().unwrap();
        let Some(p) = st.pending.remove(key) else { return };
        let decision = if allow { Decision::Allowed } else { Decision::Denied };
        st.known.insert(key.to_owned(), (p.request.client.clone(), decision));
        drop(st);
        let _ = p.tx.send(Some(allow));
        if allow {
            let _ = app.emit(EVENT_CLIENT_CONNECTED, &p.request.client);
        }
    }

    pub(crate) fn connected(&self) -> Vec<ConnectedClient> {
        let st = self.state.lock().unwrap();
        let mut v: Vec<_> =
            st.known.values().filter(|(_, d)| *d == Decision::Allowed).map(|(c, _)| c.clone()).collect();
        v.sort_by_key(|c| c.first_seen);
        v
    }

    pub(crate) fn pending(&self) -> Vec<AccessRequest> {
        let st = self.state.lock().unwrap();
        let mut v: Vec<_> = st.pending.values().map(|p| p.request.clone()).collect();
        v.sort_by_key(|r| r.id);
        v
    }

    /// Forgets every client and denies pending requests, e.g. after the token
    /// was rotated or the server stopped.
    pub(crate) fn reset(&self) {
        let mut st = self.state.lock().unwrap();
        for (_, p) in st.pending.drain() {
            let _ = p.tx.send(Some(false));
        }
        st.known.clear();
        st.aliases.clear();
    }
}

fn denied(c: &ConnectedClient) -> String {
    format!("the app denied access to `{}`", c.name.as_deref().or(c.user_agent.as_deref()).unwrap_or("this client"))
}
