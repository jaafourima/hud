//! hud-ui: the Dioxus shell. It knows nothing about individual plugins; each plugin
//! contributes a [`Page`] and the shell renders a nav bar plus the selected page.

use std::sync::{Arc, Mutex, MutexGuard};

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

const CSS: &str = include_str!("style.css");

#[component]
pub fn App() -> Element {
    let state = use_context::<AppState>();
    let mut current = use_signal(|| 0usize);
    let pages = state.pages.clone();

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
            main {
                if pages.is_empty() {
                    p { class: "muted", "No plugins enabled." }
                } else {
                    PageHost { key: "{current}", index: current() }
                }
            }
        }
    }
}

/// Hosts one page in its own scope, so hooks inside a page are fresh after switching.
#[component]
fn PageHost(index: usize) -> Element {
    let state = use_context::<AppState>();
    (state.pages[index].component)()
}
