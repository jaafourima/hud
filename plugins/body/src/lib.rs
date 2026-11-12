//! Body plugin: weight and body-fat measurements, plus a smoothed weight trend.

use hud_core::{EntityDef, Manifest, Migration, Plugin, RecordId, SqlView};
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

static SERVER_MIGRATIONS: [Migration; 1] = [Migration {
    version: 1,
    sql: "CREATE TABLE body_measurement(
              user_id uuid NOT NULL, id uuid NOT NULL, hlc text NOT NULL, device_id uuid NOT NULL,
              measured_at bigint NOT NULL,  -- unix seconds, UTC
              weight_kg double precision, body_fat_pct double precision, note text,
              PRIMARY KEY (user_id, id));
          CREATE INDEX body_measurement_time ON body_measurement(user_id, measured_at);",
}];

static VIEWS: [SqlView; 1] = [SqlView {
    name: "body_measurements",
    sql: "SELECT user_id, id, to_timestamp(measured_at) AS time, weight_kg, body_fat_pct FROM body_measurement",
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
    fn entities(&self) -> &'static [EntityDef] {
        &ENTITIES
    }
    fn migrations(&self) -> &'static [Migration] {
        &MIGRATIONS
    }
    fn server_migrations(&self) -> &'static [Migration] {
        &SERVER_MIGRATIONS
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

#[cfg(feature = "ui")]
pub mod ui;

/// Chart geometry in a `width` x `height` box: raw readings and the smoothed trend,
/// x by time and y by weight. Returns `None` with no weight readings.
pub struct Chart {
    pub raw: Vec<(f64, f64)>,
    pub trend: Vec<(f64, f64)>,
    pub min_kg: f64,
    pub max_kg: f64,
}

pub fn chart(history: &[Measurement], alpha: f64, width: f64, height: f64) -> Option<Chart> {
    let pts: Vec<(i64, f64)> = history
        .iter()
        .filter_map(|m| m.weight_kg.map(|w| (m.measured_at, w)))
        .collect();
    let (first, last) = (pts.first()?.0, pts.last()?.0);
    let weights: Vec<f64> = pts.iter().map(|p| p.1).collect();
    let smooth = trend(&weights, alpha);
    let lo = weights.iter().copied().fold(f64::INFINITY, f64::min);
    let hi = weights.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let (min_kg, max_kg) = if hi - lo < 1.0 {
        (lo - 0.5, hi + 0.5)
    } else {
        (lo, hi)
    };
    let x = |t: i64| {
        if last == first {
            width / 2.0
        } else {
            (t - first) as f64 / (last - first) as f64 * width
        }
    };
    let y = |w: f64| height - (w - min_kg) / (max_kg - min_kg) * height;
    Some(Chart {
        raw: pts.iter().map(|&(t, w)| (x(t), y(w))).collect(),
        trend: pts
            .iter()
            .zip(&smooth)
            .map(|(&(t, _), &w)| (x(t), y(w)))
            .collect(),
        min_kg,
        max_kg,
    })
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
    fn chart_scales_time_and_weight_into_the_box() {
        assert!(chart(&[], 0.1, 100.0, 50.0).is_none());
        let h = [
            Measurement::new(0, Some(80.0), None, None),
            Measurement::new(10, None, Some(20.0), None), // no weight: not plotted
            Measurement::new(100, Some(90.0), None, None),
        ];
        let c = chart(&h, 1.0, 200.0, 100.0).unwrap();
        assert_eq!(
            c.raw,
            vec![(0.0, 100.0), (200.0, 0.0)],
            "lightest bottom-left, heaviest top-right"
        );
        let one = chart(&h[..1], 0.1, 200.0, 100.0).unwrap();
        assert_eq!(
            one.raw,
            vec![(100.0, 50.0)],
            "a single reading sits in the middle"
        );
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
