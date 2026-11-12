//! hud-ui: the Dioxus shell. It knows nothing about individual plugins; each plugin
//! contributes a [`Page`] and the shell renders a nav bar plus the selected page.

use std::{
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};

use dioxus::prelude::*;
use hud_store::Store;

/// One screen contributed by a plugin.
#[derive(Clone, Copy)]
pub struct Page {
    pub id: &'static str,
    pub label: &'static str,
    /// Called inside a component scope, so it may use hooks.
    pub component: fn() -> Element,
}

/// Shared app state, provided to the root component as context.
#[derive(Clone)]
pub struct AppState {
    pub store: Arc<Mutex<Store>>,
    pub pages: Arc<Vec<Page>>,
}

impl AppState {
    pub fn new(store: Store, pages: Vec<Page>) -> Self {
        Self {
            store: Arc::new(Mutex::new(store)),
            pages: Arc::new(pages),
        }
    }
}

/// Handle to the local store for use inside page components.
#[derive(Clone)]
pub struct StoreHandle(Arc<Mutex<Store>>);

impl StoreHandle {
    /// The shared store, for code that locks it itself (such as `hud-client`).
    pub fn arc(&self) -> Arc<Mutex<Store>> {
        self.0.clone()
    }
    pub fn lock(&self) -> MutexGuard<'_, Store> {
        // A panic while holding the lock can't leave the SQLite data inconsistent
        // (writes are transactions), so keep going with the inner value.
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

pub fn use_store() -> StoreHandle {
    StoreHandle(use_context::<AppState>().store)
}

mod sync_page;

const CSS: &str = include_str!("style.css");
const SYNC_INTERVAL: Duration = Duration::from_secs(30);

/// Bumped whenever stored data may have changed (a local write, or ops pulled from the
/// server). Pages read it inside their data queries so they reload.
#[derive(Clone, Copy)]
pub struct DataVersion(Signal<u32>);

impl DataVersion {
    /// Reading subscribes the current reactive scope.
    pub fn get(self) -> u32 {
        (self.0)()
    }
    pub fn bump(mut self) {
        self.0 += 1;
    }
}

pub fn use_data_version() -> DataVersion {
    use_context::<DataVersion>()
}

#[derive(Clone, Default, PartialEq)]
pub struct SyncStatus {
    pub busy: bool,
    pub ok: bool,
    pub message: Option<String>,
}

#[derive(Clone, Copy)]
pub(crate) struct SyncState(pub Signal<SyncStatus>);

/// One sync round trip, recording the outcome for the Sync page. No-op if one is running.
pub(crate) async fn run_sync(
    store: Arc<Mutex<Store>>,
    mut status: Signal<SyncStatus>,
    data: DataVersion,
) {
    if status.peek().busy {
        return;
    }
    status.write().busy = true;
    let result = hud_client::sync(&store).await;
    let now = chrono::Local::now().format("%H:%M:%S");
    *status.write() = match result {
        Ok(r) => {
            if r.pulled > 0 {
                data.bump();
            }
            SyncStatus {
                busy: false,
                ok: true,
                message: Some(format!(
                    "Synced at {now}: sent {}, received {}",
                    r.pushed, r.pulled
                )),
            }
        }
        Err(hud_client::Error::NotConfigured) => SyncStatus::default(),
        Err(e) => SyncStatus {
            busy: false,
            ok: false,
            message: Some(format!("Sync failed at {now}: {e}")),
        },
    };
}

fn builtin_pages() -> [Page; 1] {
    [Page {
        id: "sync",
        label: "Sync",
        component: sync_page::sync_page,
    }]
}

fn all_pages(state: &AppState) -> Vec<Page> {
    state.pages.iter().copied().chain(builtin_pages()).collect()
}

#[component]
pub fn App() -> Element {
    let state = use_context::<AppState>();
    let mut current = use_signal(|| 0usize);
    let pages = all_pages(&state);

    let version = use_signal(|| 0u32);
    let data = use_context_provider(|| DataVersion(version));
    let status = use_signal(SyncStatus::default);
    use_context_provider(|| SyncState(status));

    // Background sync while connected to a server.
    let store = state.store.clone();
    use_future(move || {
        let store = store.clone();
        async move {
            loop {
                if hud_client::is_connected(&store) {
                    run_sync(store.clone(), status, data).await;
                }
                tokio::time::sleep(SYNC_INTERVAL).await;
            }
        }
    });

    rsx! {
        style { "{CSS}" }
        div { class: "shell",
            nav {
                h1 { "hud" }
                for (i , page) in pages.iter().enumerate() {
                    button {
                        key: "{page.id}",
                        class: if current() == i { "active" },
                        onclick: move |_| current.set(i),
                        "{page.label}"
                    }
                }
            }
            main { PageHost { key: "{current}", index: current() } }
        }
    }
}

/// Hosts one page in its own scope, so hooks inside a page are fresh after switching.
#[component]
fn PageHost(index: usize) -> Element {
    let state = use_context::<AppState>();
    (all_pages(&state)[index].component)()
}

#[cfg(test)]
mod tests {
    use hud_core::Registry;

    use super::*;

    fn render(store: Store, pages: Vec<Page>) -> String {
        let mut vdom = VirtualDom::new(App).with_root_context(AppState::new(store, pages));
        vdom.rebuild_in_place();
        dioxus_ssr::render(&vdom)
    }

    fn store() -> Store {
        Store::open_in_memory(Registry::new(vec![]).unwrap()).unwrap()
    }

    #[test]
    fn sync_page_is_always_available_and_offers_connecting_when_unconfigured() {
        let html = render(store(), vec![]); // with no plugin pages, Sync is the first page
        for want in [
            "Sync",
            "Server address",
            "Create a new account",
            "works offline",
        ] {
            assert!(html.contains(want), "missing `{want}` in: {html}");
        }
        assert!(!html.contains("Sync now"));
    }

    #[test]
    fn sync_page_shows_the_server_once_connected() {
        let s = store();
        s.set_setting("sync.url", "https://hud.example.com")
            .unwrap();
        s.set_setting("sync.token", "t").unwrap();
        let html = render(s, vec![]);
        for want in [
            "Connected to",
            "https://hud.example.com",
            "Sync now",
            "Disconnect",
        ] {
            assert!(html.contains(want), "missing `{want}` in: {html}");
        }
        assert!(!html.contains("Server address"));
    }
}
