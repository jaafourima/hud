//! End-to-end tests: real `hud-store` clients talking HTTP to the real server on a real
//! Postgres. Set HUD_TEST_DATABASE_URL to e.g. `postgres://hud@127.0.0.1:5432` (a role that
//! can create databases and roles); without it these tests print a notice and pass.

use hud_core::{DeviceId, HlcClock, Op, RecordId, Registry};
use hud_plugin_body::{Body, Measurement, history, remove, save};
use hud_server::{AppState, db, router};
use hud_store::Store;
use hud_sync::{Credentials, PairRequest, PairResponse, SyncRequest, SyncResponse};
use serde::Serialize;
use serde_json::{Value, json};
use tokio_postgres::NoTls;
use uuid::Uuid;

const PASSWORD: &str = "correct horse battery";

struct Harness {
    base: String,
    http: reqwest::Client,
    admin: tokio_postgres::Client,
    db_url: String,
    db_name: String,
    admin_url: String,
}

fn registry() -> Registry {
    Registry::new(vec![Box::new(Body)]).unwrap()
}

fn store() -> Store {
    Store::open_in_memory(registry()).unwrap()
}

async fn n(c: &tokio_postgres::Client, sql: &str) -> i64 {
    c.query_one(sql, &[]).await.unwrap().get(0)
}

async fn connect(url: &str) -> tokio_postgres::Client {
    let (client, conn) = tokio_postgres::connect(url, NoTls)
        .await
        .expect("cannot reach test Postgres");
    tokio::spawn(conn);
    client
}

impl Harness {
    async fn start(allow_registration: bool) -> Option<Harness> {
        let Ok(base_url) = std::env::var("HUD_TEST_DATABASE_URL") else {
            eprintln!("HUD_TEST_DATABASE_URL not set: skipping server integration test");
            return None;
        };
        let admin_url = format!("{base_url}/postgres");
        let admin = connect(&admin_url).await;
        let db_name = format!("hud_test_{}", Uuid::new_v4().simple());
        admin
            .batch_execute(&format!("CREATE DATABASE {db_name}"))
            .await
            .unwrap();
        let db_url = format!("{base_url}/{db_name}");

        let pool = db::connect(&db_url).unwrap();
        db::migrate(&pool, &registry()).await.unwrap();
        db::migrate(&pool, &registry()).await.unwrap(); // running twice must be harmless
        let mut state = AppState::new(pool, registry(), allow_registration).unwrap();
        state.grafana_db_host = "127.0.0.1".to_owned().into();
        state.grafana_db_port = base_url
            .rsplit(':')
            .next()
            .and_then(|p| p.parse().ok())
            .unwrap_or(5432);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, router(state)).await.unwrap() });
        Some(Harness {
            base,
            http: reqwest::Client::new(),
            admin,
            db_url,
            db_name,
            admin_url,
        })
    }

    async fn post<T: Serialize>(&self, path: &str, token: Option<&str>, body: &T) -> (u16, Value) {
        let mut req = self.http.post(format!("{}{path}", self.base)).json(body);
        if let Some(t) = token {
            req = req.bearer_auth(t);
        }
        let resp = req.send().await.unwrap();
        let status = resp.status().as_u16();
        (status, resp.json().await.unwrap_or(Value::Null))
    }

    async fn register(&self, user: &str) -> u16 {
        self.post(
            "/api/register",
            None,
            &Credentials {
                username: user.into(),
                password: PASSWORD.into(),
            },
        )
        .await
        .0
    }

    /// Pair `store` as a device of `user` and return its token.
    async fn pair(&self, user: &str, password: &str, store: &Store) -> (u16, Option<String>) {
        let req = PairRequest {
            username: user.into(),
            password: password.into(),
            device_name: "test device".into(),
            device_id: store.device_id(),
        };
        let (status, body) = self.post("/api/devices", None, &req).await;
        (
            status,
            serde_json::from_value::<PairResponse>(body)
                .ok()
                .map(|r| r.token),
        )
    }

    async fn paired(&self, user: &str, store: &Store) -> String {
        self.pair(user, PASSWORD, store)
            .await
            .1
            .expect("pairing failed")
    }

    async fn sync_raw(&self, token: &str, cursor: i64, ops: Vec<Op>) -> (u16, Value) {
        self.post("/api/sync", Some(token), &SyncRequest { cursor, ops })
            .await
    }

    /// One full round trip for a client store: push its outbox, apply what comes back.
    async fn sync(&self, token: &str, store: &mut Store, cursor: &mut i64) {
        let pending = store.pending_ops(500).unwrap();
        let ops = pending.iter().map(|(_, op)| op.clone()).collect();
        let (status, body) = self.sync_raw(token, *cursor, ops).await;
        assert_eq!(status, 200, "sync failed: {body}");
        let resp: SyncResponse = serde_json::from_value(body).unwrap();
        store.apply_remote(&resp.ops).unwrap();
        if let Some((seq, _)) = pending.last() {
            store.mark_synced(*seq).unwrap();
        }
        *cursor = resp.cursor;
    }

    async fn count(&self, sql: &str) -> i64 {
        connect(&self.db_url)
            .await
            .query_one(sql, &[])
            .await
            .unwrap()
            .get(0)
    }

    async fn cleanup(self) {
        let roles: Vec<String> = connect(&self.db_url)
            .await
            .query("SELECT role_name FROM hud_sys.grafana_access", &[])
            .await
            .unwrap()
            .iter()
            .map(|r| r.get(0))
            .collect();
        let admin = connect(&self.admin_url).await;
        admin
            .batch_execute(&format!("DROP DATABASE {} WITH (FORCE)", self.db_name))
            .await
            .unwrap();
        for r in roles {
            admin
                .batch_execute(&format!("DROP ROLE IF EXISTS {r}"))
                .await
                .unwrap();
        }
        let _ = &self.admin;
    }
}

#[tokio::test]
async fn two_devices_of_one_user_converge() {
    let Some(h) = Harness::start(false).await else {
        return;
    };
    assert_eq!(h.register("alice").await, 201);
    let (mut a, mut b) = (store(), store());
    let (ta, tb) = (h.paired("alice", &a).await, h.paired("alice", &b).await);
    let (mut ca, mut cb) = (0, 0);

    let kept = Measurement::new(1_000, Some(80.0), None, None);
    let doomed = Measurement::new(2_000, Some(81.0), Some(18.0), Some("x".into()));
    save(&mut a, &kept).unwrap();
    save(&mut a, &doomed).unwrap();
    h.sync(&ta, &mut a, &mut ca).await;
    h.sync(&tb, &mut b, &mut cb).await;
    assert_eq!(history(&b).unwrap(), vec![kept.clone(), doomed.clone()]);
    assert_eq!(h.count("SELECT count(*) FROM body_measurement").await, 2);

    // B edits one record and deletes the other; A picks both up.
    let edited = Measurement {
        weight_kg: Some(79.5),
        ..kept.clone()
    };
    save(&mut b, &edited).unwrap();
    remove(&mut b, doomed.id).unwrap();
    h.sync(&tb, &mut b, &mut cb).await;
    h.sync(&ta, &mut a, &mut ca).await;
    assert_eq!(history(&a).unwrap(), vec![edited.clone()]);
    assert_eq!(history(&b).unwrap(), vec![edited]);
    assert_eq!(h.count("SELECT count(*) FROM body_measurement").await, 1);
    assert!(
        a.pending_ops(10).unwrap().is_empty(),
        "acknowledged ops leave the outbox"
    );
    h.cleanup().await;
}

#[tokio::test]
async fn users_cannot_see_each_others_data() {
    let Some(h) = Harness::start(true).await else {
        return;
    };
    h.register("alice").await;
    h.register("bob").await;
    let (mut a, mut b) = (store(), store());
    let (ta, tb) = (h.paired("alice", &a).await, h.paired("bob", &b).await);
    let (mut ca, mut cb) = (0, 0);
    save(&mut a, &Measurement::new(1_000, Some(70.0), None, None)).unwrap();
    h.sync(&ta, &mut a, &mut ca).await;
    h.sync(&tb, &mut b, &mut cb).await;
    assert!(
        history(&b).unwrap().is_empty(),
        "bob must not receive alice's data"
    );

    // Row-level security itself, independent of the API's own WHERE clauses: as the app
    // role, rows are visible only when app.user_id names their owner.
    let alice: Uuid = h.admin_user_id(&h, "alice").await;
    let bob: Uuid = h.admin_user_id(&h, "bob").await;
    let db = connect(&h.db_url).await;
    for (who, expected) in [(None, 0), (Some(alice), 1), (Some(bob), 0)] {
        db.batch_execute("BEGIN; SET LOCAL ROLE hud_app")
            .await
            .unwrap();
        if let Some(id) = who {
            db.execute(
                "SELECT set_config('app.user_id', $1, true)",
                &[&id.to_string()],
            )
            .await
            .unwrap();
        }
        let n: i64 = db
            .query_one("SELECT count(*) FROM body_measurement", &[])
            .await
            .unwrap()
            .get(0);
        let ops: i64 = db
            .query_one("SELECT count(*) FROM ops", &[])
            .await
            .unwrap()
            .get(0);
        db.batch_execute("ROLLBACK").await.unwrap();
        assert_eq!((n, ops), (expected, expected), "as {who:?}");
    }
    h.cleanup().await;
}

impl Harness {
    async fn admin_user_id(&self, _h: &Harness, name: &str) -> Uuid {
        connect(&self.db_url)
            .await
            .query_one("SELECT id FROM users WHERE username = $1", &[&name])
            .await
            .unwrap()
            .get(0)
    }
}

#[tokio::test]
async fn account_and_token_rules() {
    let Some(h) = Harness::start(false).await else {
        return;
    };
    let creds = |u: &str, p: &str| Credentials {
        username: u.into(),
        password: p.into(),
    };
    assert_eq!(
        h.post("/api/register", None, &creds("al", PASSWORD))
            .await
            .0,
        400,
        "short username"
    );
    assert_eq!(
        h.post("/api/register", None, &creds("alice", "short"))
            .await
            .0,
        400,
        "short password"
    );
    assert_eq!(h.register("alice").await, 201);
    assert_eq!(
        h.register("bob").await,
        403,
        "registration is closed once the first user exists"
    );

    let s = store();
    assert_eq!(h.pair("alice", "wrong password!", &s).await.0, 401);
    assert_eq!(
        h.pair("nobody", PASSWORD, &s).await.0,
        401,
        "unknown user looks the same as a wrong password"
    );
    let token = h.paired("ALICE", &s).await; // usernames are case-insensitive
    assert_eq!(h.sync_raw(&token, 0, vec![]).await.0, 200);
    assert_eq!(h.sync_raw("not-a-token", 0, vec![]).await.0, 401);
    let (status, _) = h
        .post(
            "/api/sync",
            None,
            &SyncRequest {
                cursor: 0,
                ops: vec![],
            },
        )
        .await;
    assert_eq!(status, 401, "no Authorization header");

    // Re-pairing the same device replaces its token; the old one stops working.
    let token2 = h.paired("alice", &s).await;
    assert_eq!(h.sync_raw(&token, 0, vec![]).await.0, 401);
    assert_eq!(h.sync_raw(&token2, 0, vec![]).await.0, 200);
    h.cleanup().await;

    let Some(h) = Harness::start(true).await else {
        return;
    };
    assert_eq!(h.register("alice").await, 201);
    assert_eq!(
        h.register("Alice").await,
        409,
        "duplicate username, ignoring case"
    );
    // A device id already used by another account cannot be claimed.
    h.register("bob").await;
    let s = store();
    h.paired("alice", &s).await;
    assert_eq!(h.pair("bob", PASSWORD, &s).await.0, 409);
    h.cleanup().await;
}

fn op(
    entity: &str,
    id: RecordId,
    fields: Value,
    hlc_ms: u64,
    device: DeviceId,
    deleted: bool,
) -> Op {
    Op {
        record_id: id,
        entity: entity.into(),
        fields: fields.as_object().unwrap().clone(),
        hlc: HlcClock::new(device).tick(hlc_ms),
        device_id: device,
        deleted,
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

#[tokio::test]
async fn bad_ops_are_rejected_and_roll_back_the_whole_request() {
    let Some(h) = Harness::start(false).await else {
        return;
    };
    h.register("alice").await;
    let s = store();
    let t = h.paired("alice", &s).await;
    let dev = s.device_id();
    let good = |ms| {
        op(
            "body_measurement",
            RecordId::new(),
            json!({"measured_at": 5, "weight_kg": 70.0}),
            ms,
            dev,
            false,
        )
    };

    let cases = [
        (
            "unknown entity",
            op(
                "nope_thing",
                RecordId::new(),
                json!({}),
                now_ms(),
                dev,
                false,
            ),
        ),
        (
            "unknown field",
            op(
                "body_measurement",
                RecordId::new(),
                json!({"colour": "red"}),
                now_ms(),
                dev,
                false,
            ),
        ),
        (
            "someone else's device id",
            op(
                "body_measurement",
                RecordId::new(),
                json!({"measured_at": 1}),
                now_ms(),
                DeviceId::new(),
                false,
            ),
        ),
        (
            "far-future timestamp",
            good(now_ms() + 3 * 24 * 3600 * 1000),
        ),
        (
            "wrong value type",
            op(
                "body_measurement",
                RecordId::new(),
                json!({"measured_at": "not a number"}),
                now_ms(),
                dev,
                false,
            ),
        ),
        (
            "missing required column",
            op(
                "body_measurement",
                RecordId::new(),
                json!({"weight_kg": 1.0}),
                now_ms(),
                dev,
                false,
            ),
        ),
    ];
    for (name, bad) in cases {
        // A valid op rides along; it must not be stored when the request as a whole fails.
        let (status, _) = h.sync_raw(&t, 0, vec![good(now_ms()), bad]).await;
        assert_eq!(status, 400, "{name}");
    }
    assert_eq!(h.count("SELECT count(*) FROM ops").await, 0);
    assert_eq!(h.count("SELECT count(*) FROM body_measurement").await, 0);

    let too_many = (0..501).map(|i| good(now_ms() + i)).collect();
    assert_eq!(h.sync_raw(&t, 0, too_many).await.0, 400);
    assert_eq!(h.sync_raw(&t, -1, vec![]).await.0, 400);
    h.cleanup().await;
}

#[tokio::test]
async fn merge_is_last_writer_wins_and_pushes_are_idempotent() {
    let Some(h) = Harness::start(false).await else {
        return;
    };
    h.register("alice").await;
    let s = store();
    let t = h.paired("alice", &s).await;
    let dev = s.device_id();
    let id = RecordId::new();
    let t0 = now_ms();
    let write = |w: f64, ms| {
        op(
            "body_measurement",
            id,
            json!({"measured_at": 1, "weight_kg": w}),
            t0 + ms,
            dev,
            false,
        )
    };
    let weight = || h.count("SELECT coalesce(max(weight_kg), -1)::bigint FROM body_measurement");

    h.sync_raw(&t, 0, vec![write(90.0, 20)]).await;
    h.sync_raw(&t, 0, vec![write(80.0, 10)]).await; // older edit arriving late
    assert_eq!(
        weight().await,
        90,
        "older op must not overwrite a newer one"
    );
    h.sync_raw(&t, 0, vec![write(90.0, 20), write(90.0, 20)])
        .await; // replay
    assert_eq!(
        h.count("SELECT count(*) FROM ops").await,
        2,
        "duplicates are not logged twice"
    );

    h.sync_raw(
        &t,
        0,
        vec![op("body_measurement", id, json!({}), t0 + 30, dev, true)],
    )
    .await;
    assert_eq!(h.count("SELECT count(*) FROM body_measurement").await, 0);
    h.sync_raw(&t, 0, vec![write(100.0, 25)]).await; // edit older than the delete
    assert_eq!(
        h.count("SELECT count(*) FROM body_measurement").await,
        0,
        "a late edit must not resurrect a deleted record"
    );
    h.cleanup().await;
}

#[tokio::test]
async fn pulls_are_paged_by_cursor() {
    let Some(h) = Harness::start(false).await else {
        return;
    };
    h.register("alice").await;
    let (a, b) = (store(), store());
    let (ta, tb) = (h.paired("alice", &a).await, h.paired("alice", &b).await);
    let t0 = now_ms();
    let batch = |from: u64| -> Vec<Op> {
        (from..from + 300)
            .map(|i| {
                op(
                    "body_measurement",
                    RecordId::new(),
                    json!({"measured_at": i}),
                    t0 + i,
                    a.device_id(),
                    false,
                )
            })
            .collect()
    };
    h.sync_raw(&ta, 0, batch(0)).await;
    h.sync_raw(&ta, 0, batch(300)).await;

    let (_, first) = h.sync_raw(&tb, 0, vec![]).await;
    let first: SyncResponse = serde_json::from_value(first).unwrap();
    assert_eq!((first.ops.len(), first.more), (500, true));
    let (_, rest) = h.sync_raw(&tb, first.cursor, vec![]).await;
    let rest: SyncResponse = serde_json::from_value(rest).unwrap();
    assert_eq!((rest.ops.len(), rest.more), (100, false));
    let (_, none) = h.sync_raw(&tb, rest.cursor, vec![]).await;
    assert!(
        serde_json::from_value::<SyncResponse>(none)
            .unwrap()
            .ops
            .is_empty()
    );
    h.cleanup().await;
}

#[tokio::test]
async fn grafana_login_reads_only_the_owners_views() {
    let Some(h) = Harness::start(true).await else {
        return;
    };
    h.register("alice").await;
    h.register("bob").await;
    let (mut a, mut b) = (store(), store());
    let (ta, tb) = (h.paired("alice", &a).await, h.paired("bob", &b).await);
    let (mut ca, mut cb) = (0, 0);
    for ms in [1, 2, 3] {
        save(&mut a, &Measurement::new(ms, Some(70.0), None, None)).unwrap();
    }
    save(&mut b, &Measurement::new(9, Some(90.0), None, None)).unwrap();
    h.sync(&ta, &mut a, &mut ca).await;
    h.sync(&tb, &mut b, &mut cb).await;

    let grafana = |token: String| {
        let h = &h;
        async move {
            let (status, body) = h.post("/api/grafana", Some(&token), &json!({})).await;
            assert_eq!(status, 200, "{body}");
            let url = format!(
                "postgres://{}:{}@{}:{}/{}",
                body["user"].as_str().unwrap(),
                body["password"].as_str().unwrap(),
                body["host"].as_str().unwrap(),
                body["port"],
                body["database"].as_str().unwrap()
            );
            (url, body["password"].as_str().unwrap().to_owned())
        }
    };
    let (alice_url, alice_pw) = grafana(ta.clone()).await;
    let (bob_url, _) = grafana(tb.clone()).await;

    let alice_db = connect(&alice_url).await;
    assert_eq!(
        n(&alice_db, "SELECT count(*) FROM grafana.body_measurements").await,
        3
    );
    assert_eq!(
        n(
            &connect(&bob_url).await,
            "SELECT count(*) FROM grafana.body_measurements"
        )
        .await,
        1
    );
    for forbidden in [
        "SELECT count(*) FROM body_measurement",
        "SELECT count(*) FROM ops",
        "SELECT count(*) FROM users",
        "SELECT count(*) FROM hud_sys.grafana_access",
    ] {
        let err = alice_db
            .query_one(forbidden, &[])
            .await
            .expect_err(forbidden);
        assert_eq!(
            err.code().map(|c| c.code()),
            Some("42501"),
            "{forbidden}: {err}"
        );
    }
    // Setting app.user_id (which RLS uses for the API) gives no way into another user's views.
    alice_db
        .batch_execute(&format!(
            "SET app.user_id = '{}'",
            h.admin_user_id(&h, "bob").await
        ))
        .await
        .unwrap();
    assert_eq!(
        n(&alice_db, "SELECT count(*) FROM grafana.body_measurements").await,
        3
    );

    // Asking again rotates the password; the old one stops working.
    let (_, new_pw) = grafana(ta.clone()).await;
    assert_ne!(alice_pw, new_pw);
    assert!(tokio_postgres::connect(&alice_url, NoTls).await.is_err());
    h.cleanup().await;
}
