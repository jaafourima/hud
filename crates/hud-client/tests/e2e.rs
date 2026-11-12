//! hud-client against a real hud-server and Postgres. Needs HUD_TEST_DATABASE_URL
//! (see crates/hud-server/tests/api.rs); without it the test prints a notice and passes.

use std::sync::Mutex;

use hud_client::{Error, Report, connect, disconnect, is_connected, sync};
use hud_core::Registry;
use hud_plugin_body::{Body, Measurement, history, save};
use hud_server::{AppState, db, router};
use hud_store::Store;
use tokio_postgres::NoTls;
use uuid::Uuid;

fn store() -> Mutex<Store> {
    Mutex::new(Store::open_in_memory(Registry::new(vec![Box::new(Body)]).unwrap()).unwrap())
}

fn registry() -> Registry {
    Registry::new(vec![Box::new(Body)]).unwrap()
}

#[tokio::test]
async fn devices_sync_through_the_client_library() {
    let Ok(base_url) = std::env::var("HUD_TEST_DATABASE_URL") else {
        eprintln!("HUD_TEST_DATABASE_URL not set: skipping client e2e test");
        return;
    };
    let admin_url = format!("{base_url}/postgres");
    let (admin, conn) = tokio_postgres::connect(&admin_url, NoTls).await.unwrap();
    tokio::spawn(conn);
    let name = format!("hud_test_{}", Uuid::new_v4().simple());
    admin
        .batch_execute(&format!("CREATE DATABASE {name}"))
        .await
        .unwrap();

    let pool = db::connect(&format!("{base_url}/{name}")).unwrap();
    db::migrate(&pool, &registry()).await.unwrap();
    let state = AppState::new(pool, registry(), false).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap()); // trailing slash is tolerated
    tokio::spawn(async move { axum::serve(listener, router(state)).await.unwrap() });

    let (a, b) = (store(), store());
    assert!(matches!(sync(&a).await, Err(Error::NotConfigured)));
    assert!(!is_connected(&a));

    // The server's own message comes through, and nothing is saved on failure.
    connect(&a, &url, "alice", "correct horse battery", "Laptop", true)
        .await
        .unwrap();
    assert!(is_connected(&a));
    let err = connect(&b, &url, "alice", "nope nope nope", "Phone", false)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("invalid credentials"), "{err}");
    assert!(!is_connected(&b));
    connect(&b, &url, "alice", "correct horse battery", "Phone", false)
        .await
        .unwrap();

    // More than one request's worth (500) of changes, so push and pull both page.
    for i in 0..620 {
        save(
            &mut a.lock().unwrap(),
            &Measurement::new(i, Some(70.0 + (i % 5) as f64), None, None),
        )
        .unwrap();
    }
    assert_eq!(
        sync(&a).await.unwrap(),
        Report {
            pushed: 620,
            pulled: 0
        }
    );
    assert_eq!(
        sync(&b).await.unwrap(),
        Report {
            pushed: 0,
            pulled: 620
        }
    );
    assert_eq!(
        history(&b.lock().unwrap()).unwrap(),
        history(&a.lock().unwrap()).unwrap()
    );
    assert_eq!(
        sync(&b).await.unwrap(),
        Report::default(),
        "nothing new: nothing moves"
    );

    // B's own changes come back to A, and A does not receive an echo of its own ops.
    let extra = Measurement::new(9_999, Some(66.6), None, Some("from phone".into()));
    save(&mut b.lock().unwrap(), &extra).unwrap();
    assert_eq!(
        sync(&b).await.unwrap(),
        Report {
            pushed: 1,
            pulled: 0
        }
    );
    assert_eq!(
        sync(&a).await.unwrap(),
        Report {
            pushed: 0,
            pulled: 1
        }
    );
    assert!(history(&a.lock().unwrap()).unwrap().contains(&extra));

    disconnect(&a).unwrap();
    assert!(!is_connected(&a));
    assert!(matches!(sync(&a).await, Err(Error::NotConfigured)));
    assert_eq!(
        history(&a.lock().unwrap()).unwrap().len(),
        621,
        "local data survives disconnecting"
    );

    let (admin2, conn2) = tokio_postgres::connect(&admin_url, NoTls).await.unwrap();
    tokio::spawn(conn2);
    admin2
        .batch_execute(&format!("DROP DATABASE {name} WITH (FORCE)"))
        .await
        .unwrap();
}
