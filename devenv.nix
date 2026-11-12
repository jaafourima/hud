{ pkgs, lib, ... }:

let
  # Libraries Dioxus desktop (wry/tao) links against, plus openssl for the HTTP client.
  guiLibs = with pkgs; [ webkitgtk_4_1 gtk3 libsoup_3 glib cairo pango gdk-pixbuf librsvg xdotool libayatana-appindicator openssl ];
  pg = "postgres://$USER@127.0.0.1:5432";
in
{
  languages.rust = {
    enable = true;
    channel = "stable"; # the workspace needs Rust >= 1.89
    components = [ "rustc" "cargo" "clippy" "rustfmt" "rust-analyzer" ];
  };

  packages = [ pkgs.pkg-config pkgs.cargo-deny pkgs.python3 pkgs.postgresql_17 ] ++ guiLibs;

  # Lets the desktop app find GTK/WebKit when run from this shell.
  env.LD_LIBRARY_PATH = lib.makeLibraryPath guiLibs;

  # Local Postgres for development and for the server/client integration tests.
  services.postgres = {
    enable = true;
    package = pkgs.postgresql_17;
    listen_addresses = "127.0.0.1";
    port = 5432;
    initialDatabases = [ { name = "hud"; } ];
  };

  enterShell = ''
    export HUD_TEST_DATABASE_URL="${pg}"   # tests create and drop throwaway databases here
    export HUD_DATABASE_URL="${pg}/hud"
  '';

  # `devenv up` runs Postgres and the server together.
  processes.hud-server.exec = ''
    until pg_isready -h 127.0.0.1 -p 5432 -q; do sleep 1; done
    HUD_LISTEN=127.0.0.1:8080 exec cargo run -p hud-server
  '';

  scripts.hud-desktop.exec = "cargo run -p hud-desktop";
  scripts.hud-check.exec = ''
    cargo fmt --all --check && \
    cargo clippy --workspace --all-targets --all-features -- -D warnings && \
    cargo deny check && \
    cargo test --workspace --all-features
  '';

  # `devenv test`: starts Postgres, then runs the whole suite.
  enterTest = ''
    wait_for_port 5432
    export HUD_TEST_DATABASE_URL="${pg}"
    cargo test --workspace --all-features
  '';
}
