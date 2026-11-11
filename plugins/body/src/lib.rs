//! Body plugin: weight and body-fat measurements, plus a smoothed weight trend.

use hud_core::{
    EntityDef, Manifest, Migration, Plugin, RecordId, SettingDef, SettingKind, SqlView,
};
use hud_store::Store;
use serde_json::{Value, json};

pub const ENTITY: &str = "body_measurement";

pub struct Body;

static ENTITIES: [EntityDef; 1] = [EntityDef {
    name: ENTITY,
    table: "body_measurement",
    columns: &["measured_at", "weight_kg", "body_fat_pct", "note"],
}];

static MIGRATIONS: [Migration; 1] = [Migration {
    version: 1,
    sql: "CREATE TABLE body_measurement(
              id TEXT PRIMARY KEY, hlc TEXT NOT NULL, device_id TEXT NOT NULL,
              measured_at INTEGER NOT NULL,  -- unix seconds, UTC
              weight_kg REAL, body_fat_pct REAL, note TEXT);
          CREATE INDEX body_measurement_time ON body_measurement(measured_at);",
}];

static VIEWS: [SqlView; 1] = [SqlView {
    name: "body_measurements",
    sql: "SELECT id, to_timestamp(measured_at) AS time, weight_kg, body_fat_pct FROM body_measurement",
}];

impl Plugin for Body {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: "body",
            name: "Body",
            version: env!("CARGO_PKG_VERSION"),
            depends_on: &[],
        }
    }
    fn settings(&self) -> Vec<SettingDef> {
        vec![
            SettingDef {
                key: "weight_unit",
                label: "Weight unit (kg or lb)",
                kind: SettingKind::Text,
                default: json!("kg"),
            },
            SettingDef {
                key: "trend_smoothing",
                label: "Trend smoothing (0.05 smooth .. 1 raw)",
                kind: SettingKind::Number,
                default: json!(0.1),
            },
        ]
    }
    fn entities(&self) -> &'static [EntityDef] {
        &ENTITIES
    }
    fn migrations(&self) -> &'static [Migration] {
        &MIGRATIONS
    }
    fn grafana_views(&self) -> &'static [SqlView] {
        &VIEWS
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Measurement {
    pub id: RecordId,
    /// Unix seconds, UTC.
    pub measured_at: i64,
    pub weight_kg: Option<f64>,
    pub body_fat_pct: Option<f64>,
    pub note: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("enter a weight or a body-fat percentage")]
    Empty,
    #[error("weight must be between 1 and 700 kg")]
    BadWeight,
    #[error("body fat must be between 1 and 80 %")]
    BadBodyFat,
    #[error(transparent)]
    Store(#[from] hud_store::Error),
}

impl Measurement {
    pub fn new(
        measured_at: i64,
        weight_kg: Option<f64>,
        body_fat_pct: Option<f64>,
        note: Option<String>,
    ) -> Self {
        Self {
            id: RecordId::new(),
            measured_at,
            weight_kg,
            body_fat_pct,
            note,
        }
    }

    fn validate(&self) -> Result<(), Error> {
        if self.weight_kg.is_none() && self.body_fat_pct.is_none() {
            return Err(Error::Empty);
        }
        if self.weight_kg.is_some_and(|w| !(1.0..=700.0).contains(&w)) {
            return Err(Error::BadWeight);
        }
        if self
            .body_fat_pct
            .is_some_and(|b| !(1.0..=80.0).contains(&b))
        {
            return Err(Error::BadBodyFat);
        }
        Ok(())
    }
}

/// Save a measurement (new, or an edit when `m.id` already exists).
pub fn save(store: &mut Store, m: &Measurement) -> Result<(), Error> {
    m.validate()?;
    let fields = json!({
        "measured_at": m.measured_at, "weight_kg": m.weight_kg,
        "body_fat_pct": m.body_fat_pct, "note": m.note,
    });
    let Value::Object(fields) = fields else {
        unreachable!("json! object literal")
    };
    store.upsert(ENTITY, m.id, fields)?;
    Ok(())
}

pub fn remove(store: &mut Store, id: RecordId) -> Result<(), Error> {
    store.delete(ENTITY, id)?;
    Ok(())
}

/// All measurements, oldest first.
pub fn history(store: &Store) -> Result<Vec<Measurement>, Error> {
    let mut stmt = store
        .conn()
        .prepare("SELECT id, measured_at, weight_kg, body_fat_pct, note FROM body_measurement ORDER BY measured_at, id")
        .map_err(hud_store::Error::from)?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
            ))
        })
        .map_err(hud_store::Error::from)?;
    let mut out = Vec::new();
    for row in rows {
        let (id, measured_at, weight_kg, body_fat_pct, note) =
            row.map_err(hud_store::Error::from)?;
        let id = id
            .parse()
            .map_err(|_| hud_store::Error::Corrupt(format!("record id {id}")))?;
        out.push(Measurement {
            id,
            measured_at,
            weight_kg,
            body_fat_pct,
            note,
        });
    }
    Ok(out)
}

/// Exponentially smoothed series: `t[0] = w[0]`, `t[i] = a*w[i] + (1-a)*t[i-1]`.
/// `alpha` is clamped to (0, 1].
// ponytail: smooths per reading, ignoring gaps between dates; time-weighted smoothing if gaps distort it.
pub fn trend(weights: &[f64], alpha: f64) -> Vec<f64> {
    let a = alpha.clamp(f64::EPSILON, 1.0);
    let mut out = Vec::with_capacity(weights.len());
    for &w in weights {
        let next = out.last().map_or(w, |prev: &f64| a * w + (1.0 - a) * prev);
        out.push(next);
    }
    out
}

#[cfg(test)]
mod tests {
    use hud_core::Registry;

    use super::*;

    fn store() -> Store {
        Store::open_in_memory(Registry::new(vec![Box::new(Body)]).unwrap()).unwrap()
    }

    #[test]
    fn saves_edits_and_removes_and_orders_by_time() {
        let mut s = store();
        let later = Measurement::new(2_000, Some(80.5), None, Some("morning".into()));
        let earlier = Measurement::new(1_000, Some(81.0), Some(18.0), None);
        save(&mut s, &later).unwrap();
        save(&mut s, &earlier).unwrap();
        assert_eq!(history(&s).unwrap(), vec![earlier.clone(), later.clone()]);

        let edited = Measurement {
            weight_kg: Some(79.9),
            ..later.clone()
        };
        save(&mut s, &edited).unwrap();
        assert_eq!(history(&s).unwrap()[1].weight_kg, Some(79.9));

        remove(&mut s, earlier.id).unwrap();
        assert_eq!(history(&s).unwrap(), vec![edited]);
        assert_eq!(
            s.pending_ops(100).unwrap().len(),
            4,
            "every change is queued for sync"
        );
    }

    #[test]
    fn rejects_implausible_or_empty_entries() {
        let mut s = store();
        assert!(matches!(
            save(&mut s, &Measurement::new(1, None, None, None)),
            Err(Error::Empty)
        ));
        assert!(matches!(
            save(&mut s, &Measurement::new(1, Some(0.0), None, None)),
            Err(Error::BadWeight)
        ));
        assert!(matches!(
            save(&mut s, &Measurement::new(1, Some(f64::NAN), None, None)),
            Err(Error::BadWeight)
        ));
        assert!(matches!(
            save(&mut s, &Measurement::new(1, None, Some(120.0), None)),
            Err(Error::BadBodyFat)
        ));
        assert!(history(&s).unwrap().is_empty());
    }

    #[test]
    fn trend_smooths_toward_new_level() {
        assert_eq!(trend(&[], 0.1), Vec::<f64>::new());
        assert_eq!(trend(&[80.0, 80.0, 80.0], 0.1), vec![80.0; 3]);
        let t = trend(&[80.0, 90.0, 90.0], 0.5);
        assert_eq!(t, vec![80.0, 85.0, 87.5]);
        assert_eq!(
            trend(&[1.0, 5.0], 1.0),
            vec![1.0, 5.0],
            "alpha 1 is the raw series"
        );
    }
}
