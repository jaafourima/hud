use std::{env, fs, path::PathBuf};

use dioxus::desktop::{Config, WindowBuilder};
use hud_core::Registry;
use hud_store::Store;
use hud_ui::{App, AppState};

/// `$HUD_DATA_DIR`, else `$XDG_DATA_HOME/hud`, else `~/.local/share/hud`.
fn data_dir() -> PathBuf {
    if let Some(dir) = env::var_os("HUD_DATA_DIR") {
        return dir.into();
    }
    let base = env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env::var_os("HOME").expect("HOME is not set")).join(".local/share")
        });
    base.join("hud")
}

fn main() {
    let dir = data_dir();
    fs::create_dir_all(&dir).expect("cannot create the data directory");

    let registry =
        Registry::new(vec![Box::new(hud_plugin_body::Body)]).expect("invalid plugin set");
    let store = Store::open(dir.join("hud.db"), registry).expect("cannot open the database");
    let pages = vec![hud_plugin_body::ui::page()];

    let window = WindowBuilder::new().with_title("hud");
    dioxus::LaunchBuilder::desktop()
        .with_cfg(Config::new().with_window(window).with_menu(None))
        .with_context(AppState::new(store, pages))
        .launch(App);
}
