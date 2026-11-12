//! The built-in Sync page: connect this device to a hud server.

use dioxus::prelude::*;

use crate::{SyncState, run_sync, use_data_version, use_store};

fn default_device_name() -> String {
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|h| h.trim().to_owned())
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "desktop".to_owned())
}

pub fn sync_page() -> Element {
    let store = use_store();
    let data = use_data_version();
    let status = use_context::<SyncState>().0;

    let mut connected = use_signal({
        let store = store.clone();
        move || hud_client::server_url(&store.arc())
    });
    let mut url = use_signal(String::new);
    let mut username = use_signal(String::new);
    let mut password = use_signal(String::new);
    let mut device = use_signal(default_device_name);
    let mut create = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);
    let mut working = use_signal(|| false);

    let on_connect = {
        let store = store.clone();
        move |e: FormEvent| {
            e.prevent_default();
            if working() {
                return;
            }
            working.set(true);
            error.set(None);
            let store = store.arc();
            spawn(async move {
                let result = hud_client::connect(
                    &store,
                    &url(),
                    &username(),
                    &password(),
                    &device(),
                    create(),
                )
                .await;
                match result {
                    Ok(()) => {
                        password.set(String::new());
                        connected.set(hud_client::server_url(&store));
                        run_sync(store, status, data).await;
                    }
                    Err(e) => error.set(Some(e.to_string())),
                }
                working.set(false);
            });
        }
    };

    let st = status();
    rsx! {
        h2 { "Sync" }
        if let Some(server) = connected() {
            div { class: "card",
                p {
                    "Connected to "
                    strong { "{server}" }
                }
                if let Some(msg) = st.message.clone() {
                    p { class: if st.ok { "muted" } else { "error" }, "{msg}" }
                }
                div { class: "row",
                    button {
                        class: "primary",
                        disabled: st.busy,
                        onclick: {
                            let store = store.clone();
                            move |_| {
                                spawn(run_sync(store.arc(), status, data));
                            }
                        },
                        if st.busy {
                            "Syncing…"
                        } else {
                            "Sync now"
                        }
                    }
                    button {
                        class: "link",
                        onclick: {
                            let store = store.clone();
                            move |_| {
                                let _ = hud_client::disconnect(&store.arc());
                                connected.set(None);
                            }
                        },
                        "Disconnect"
                    }
                }
                p { class: "muted", "Syncs automatically every 30 seconds. Your data stays on this device if you disconnect." }
            }
        } else {
            div { class: "card",
                p { class: "muted",
                    "Connect to your hud server to sync between devices. Without one, everything still works offline on this device."
                }
                form { onsubmit: on_connect,
                    label {
                        "Server address"
                        input {
                            r#type: "url",
                            placeholder: "https://hud.example.com",
                            value: "{url}",
                            oninput: move |e| url.set(e.value()),
                        }
                    }
                    label {
                        "Username"
                        input { r#type: "text", value: "{username}", oninput: move |e| username.set(e.value()) }
                    }
                    label {
                        "Password"
                        input { r#type: "password", value: "{password}", oninput: move |e| password.set(e.value()) }
                    }
                    label {
                        "This device's name"
                        input { r#type: "text", value: "{device}", oninput: move |e| device.set(e.value()) }
                    }
                    label { class: "check",
                        input {
                            r#type: "checkbox",
                            checked: create(),
                            onchange: move |e| create.set(e.checked()),
                        }
                        "Create a new account"
                    }
                    button { class: "primary", r#type: "submit", disabled: working(),
                        if working() {
                            "Connecting…"
                        } else {
                            "Connect"
                        }
                    }
                }
                if let Some(msg) = error() {
                    p { class: "error", "{msg}" }
                }
            }
        }
    }
}
