use hud_core::{EntityDef, HlcClock, Manifest, Migration, Plugin, RecordId, Registry};
use proptest::prelude::*;
use serde_json::{Map, Value, json};

use super::*;

struct Items;
static ENTITIES: [EntityDef; 1] = [EntityDef {
    name: "test_item",
    table: "test_item",
    columns: &["name", "qty"],
}];
static MIGRATIONS: [Migration; 1] = [Migration {
    version: 1,
    sql: "CREATE TABLE test_item(id TEXT PRIMARY KEY, hlc TEXT NOT NULL, device_id TEXT NOT NULL, name TEXT, qty INTEGER);",
}];

impl Plugin for Items {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: "test",
            name: "Test",
            version: "0",
            depends_on: &[],
        }
    }
    fn entities(&self) -> &'static [EntityDef] {
        &ENTITIES
    }
    fn migrations(&self) -> &'static [Migration] {
        &MIGRATIONS
    }
}

fn registry() -> Registry {
    Registry::new(vec![Box::new(Items)]).unwrap()
}

fn store() -> Store {
    Store::open_in_memory(registry()).unwrap()
}

fn item(name: &str, qty: i64) -> Map<String, Value> {
    json!({ "name": name, "qty": qty })
        .as_object()
        .unwrap()
        .clone()
}

/// An op as if made by another device at `wall_ms`.
fn remote(id: RecordId, fields: Map<String, Value>, deleted: bool, wall_ms: u64) -> Op {
    let node = DeviceId::new();
    Op {
        record_id: id,
        entity: "test_item".into(),
        fields,
        hlc: HlcClock::new(node).tick(wall_ms),
        device_id: node,
        deleted,
    }
}

const FAR_FUTURE: u64 = 9_000_000_000_000; // well past any local clock, so remote ops are "newer"

#[test]
fn upsert_is_stored_and_queued_until_synced() {
    let mut s = store();
    let id = RecordId::new();
    s.upsert("test_item", id, item("apple", 3)).unwrap();

    let row = s.get("test_item", id).unwrap().unwrap();
    assert_eq!(
        (row["name"].clone(), row["qty"].clone()),
        (json!("apple"), json!(3))
    );

    let pending = s.pending_ops(10).unwrap();
    assert_eq!(pending.len(), 1);
    s.mark_synced(pending[0].0).unwrap();
    assert!(s.pending_ops(10).unwrap().is_empty());
    assert!(
        s.get("test_item", id).unwrap().is_some(),
        "acknowledging must not delete data"
    );
}

#[test]
fn newer_op_wins_and_older_op_is_ignored() {
    let mut s = store();
    let id = RecordId::new();
    let newer = remote(id, item("new", 2), false, FAR_FUTURE + 10);
    let older = remote(id, item("old", 1), false, FAR_FUTURE);
    assert_eq!(s.apply_remote(&[newer]).unwrap(), 1);
    assert_eq!(s.apply_remote(&[older]).unwrap(), 0);
    assert_eq!(
        s.get("test_item", id).unwrap().unwrap()["name"],
        json!("new")
    );
}

#[test]
fn delete_leaves_a_tombstone_that_blocks_older_edits() {
    let mut s = store();
    let id = RecordId::new();
    let create = remote(id, item("a", 1), false, FAR_FUTURE);
    let delete = remote(id, Map::new(), true, FAR_FUTURE + 10);
    s.apply_remote(&[delete]).unwrap();
    assert_eq!(
        s.apply_remote(&[create]).unwrap(),
        0,
        "older create must not resurrect a deleted record"
    );
    assert!(s.get("test_item", id).unwrap().is_none());
}

#[test]
fn local_delete_removes_the_row_and_is_queued() {
    let mut s = store();
    let id = RecordId::new();
    s.upsert("test_item", id, item("a", 1)).unwrap();
    s.delete("test_item", id).unwrap();
    assert!(s.get("test_item", id).unwrap().is_none());
    assert_eq!(s.pending_ops(10).unwrap().len(), 2);
}

#[test]
fn rejects_unknown_entities_and_fields() {
    let mut s = store();
    assert!(matches!(
        s.upsert("nope", RecordId::new(), Map::new()),
        Err(Error::UnknownEntity(_))
    ));
    let bad = json!({ "colour": "red" }).as_object().unwrap().clone();
    assert!(matches!(
        s.upsert("test_item", RecordId::new(), bad),
        Err(Error::UnknownField { .. })
    ));
}

#[test]
fn reopening_keeps_data_device_id_and_a_monotonic_clock() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hud.db");
    let id = RecordId::new();

    let (device, first) = {
        let mut s = Store::open(&path, registry()).unwrap();
        let op = s.upsert("test_item", id, item("kept", 1)).unwrap();
        (s.device_id(), op.hlc)
    };
    let mut s = Store::open(&path, registry()).unwrap(); // migrations must not re-run
    assert_eq!(s.device_id(), device);
    assert_eq!(
        s.get("test_item", id).unwrap().unwrap()["name"],
        json!("kept")
    );
    let second = s.upsert("test_item", id, item("kept", 2)).unwrap();
    assert!(second.hlc > first);
}

#[test]
fn rejects_tables_missing_standard_columns() {
    struct Bad;
    static E: [EntityDef; 1] = [EntityDef {
        name: "bad_thing",
        table: "bad_thing",
        columns: &["x"],
    }];
    static M: [Migration; 1] = [Migration {
        version: 1,
        sql: "CREATE TABLE bad_thing(id TEXT PRIMARY KEY, x TEXT);",
    }];
    impl Plugin for Bad {
        fn manifest(&self) -> Manifest {
            Manifest {
                id: "bad",
                name: "Bad",
                version: "0",
                depends_on: &[],
            }
        }
        fn entities(&self) -> &'static [EntityDef] {
            &E
        }
        fn migrations(&self) -> &'static [Migration] {
            &M
        }
    }
    let r = Registry::new(vec![Box::new(Bad)]).unwrap();
    assert!(matches!(
        Store::open_in_memory(r),
        Err(Error::MissingColumn { .. })
    ));
}

type Snapshot = Vec<(String, Option<Value>)>;

fn snapshot(s: &Store, ids: &[RecordId]) -> Snapshot {
    ids.iter()
        .map(|id| {
            (
                id.to_string(),
                s.get("test_item", *id)
                    .unwrap()
                    .map(|r| json!([r["name"], r["qty"], r["hlc"]])),
            )
        })
        .collect()
}

proptest! {
    /// Any arrival order, with duplicates, converges to the same state as in-order delivery.
    #[test]
    fn merge_converges_regardless_of_order_and_duplication(
        script in proptest::collection::vec((0usize..3, any::<bool>(), 0i64..100), 1..20),
        seed in any::<u64>(),
    ) {
        let ids: Vec<RecordId> = (0..3).map(|_| RecordId::new()).collect();
        let node = DeviceId::new();
        let mut clock = HlcClock::new(node);
        let ops: Vec<Op> = script.iter().enumerate().map(|(i, (rec, del, qty))| Op {
            record_id: ids[*rec],
            entity: "test_item".into(),
            fields: if *del { Map::new() } else { item("x", *qty) },
            hlc: clock.tick(FAR_FUTURE + i as u64),
            device_id: node,
            deleted: *del,
        }).collect();

        let mut in_order = store();
        in_order.apply_remote(&ops).unwrap();

        // Deterministic shuffle plus a duplicated prefix, driven by the seed.
        let mut shuffled = ops.clone();
        let mut x = seed | 1;
        for i in (1..shuffled.len()).rev() {
            x ^= x << 13; x ^= x >> 7; x ^= x << 17;
            shuffled.swap(i, (x % (i as u64 + 1)) as usize);
        }
        shuffled.extend(ops.iter().take(ops.len() / 2).cloned());
        let mut scrambled = store();
        for chunk in shuffled.chunks(3) {
            scrambled.apply_remote(chunk).unwrap();
        }

        prop_assert_eq!(snapshot(&in_order, &ids), snapshot(&scrambled, &ids));
    }
}
