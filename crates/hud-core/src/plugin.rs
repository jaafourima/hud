//! The plugin contract. A plugin is a crate that implements [`Plugin`]; apps list the
//! plugins they include in one registry. UI contributions live in `hud-ui`
//! (a `UiPlugin` extension trait) so this crate stays free of UI dependencies.

use std::collections::{HashMap, HashSet};

use serde_json::Value;

#[derive(Clone, Copy, Debug)]
pub struct Manifest {
    /// Stable machine id, also the table-name prefix: `body`, `sleep`, ...
    pub id: &'static str,
    pub name: &'static str,
    pub version: &'static str,
    /// Plugins that must be present for this one to load.
    pub depends_on: &'static [&'static str],
}

#[derive(Clone, Copy, Debug)]
pub enum SettingKind {
    Bool,
    Number,
    Text,
}

/// One user-editable setting. The app renders a form from these, so plugins
/// don't need settings UI of their own.
#[derive(Clone, Debug)]
pub struct SettingDef {
    pub key: &'static str,
    pub label: &'static str,
    pub kind: SettingKind,
    pub default: Value,
}

/// A synced record type and the columns an [`Op`](crate::Op) may set on it.
#[derive(Clone, Copy, Debug)]
pub struct EntityDef {
    /// Entity name carried in ops, e.g. `body_weight`. Must start with the plugin id.
    pub name: &'static str,
    /// Local SQLite table the store materializes this entity into.
    pub table: &'static str,
    pub columns: &'static [&'static str],
}

#[derive(Clone, Copy, Debug)]
pub struct Migration {
    pub version: u32,
    pub sql: &'static str,
}

/// A read-only SQL view exposed to Grafana (server side, filtered per user).
#[derive(Clone, Copy, Debug)]
pub struct SqlView {
    pub name: &'static str,
    pub sql: &'static str,
}

pub trait Plugin: Send + Sync + 'static {
    fn manifest(&self) -> Manifest;
    fn settings(&self) -> Vec<SettingDef> {
        Vec::new()
    }
    fn entities(&self) -> &'static [EntityDef];
    /// Local SQLite migrations, applied in `version` order.
    fn migrations(&self) -> &'static [Migration];
    /// Postgres migrations for the server. Each entity table must have `user_id uuid NOT NULL`
    /// and primary key `(user_id, id)`; the server adds row-level security and grants itself.
    fn server_migrations(&self) -> &'static [Migration] {
        &[]
    }
    /// Grafana views. The SQL must select `user_id`; the server restricts rows to the
    /// Grafana role's owner.
    fn grafana_views(&self) -> &'static [SqlView] {
        &[]
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RegistryError {
    #[error("duplicate plugin id `{0}`")]
    Duplicate(&'static str),
    #[error("plugin `{plugin}` depends on missing plugin `{missing}`")]
    MissingDependency {
        plugin: &'static str,
        missing: &'static str,
    },
    #[error("plugin dependency cycle involving `{0}`")]
    Cycle(&'static str),
    #[error("entity `{entity}` must start with its plugin id `{plugin}`")]
    BadEntityName {
        plugin: &'static str,
        entity: &'static str,
    },
}

/// The validated set of plugins an app was built with, in dependency order.
pub struct Registry {
    plugins: Vec<Box<dyn Plugin>>,
}

impl Registry {
    pub fn new(plugins: Vec<Box<dyn Plugin>>) -> Result<Self, RegistryError> {
        let mut by_id: HashMap<&'static str, usize> = HashMap::new();
        for (i, p) in plugins.iter().enumerate() {
            let m = p.manifest();
            if by_id.insert(m.id, i).is_some() {
                return Err(RegistryError::Duplicate(m.id));
            }
            for e in p.entities() {
                if !e.name.starts_with(m.id) {
                    return Err(RegistryError::BadEntityName {
                        plugin: m.id,
                        entity: e.name,
                    });
                }
            }
        }
        for p in &plugins {
            let m = p.manifest();
            for dep in m.depends_on {
                if !by_id.contains_key(dep) {
                    return Err(RegistryError::MissingDependency {
                        plugin: m.id,
                        missing: dep,
                    });
                }
            }
        }
        // Depth-first topological sort; `visiting` catches cycles.
        let mut order = Vec::with_capacity(plugins.len());
        let mut done = HashSet::new();
        let mut visiting = HashSet::new();
        fn visit(
            i: usize,
            plugins: &[Box<dyn Plugin>],
            by_id: &HashMap<&'static str, usize>,
            done: &mut HashSet<usize>,
            visiting: &mut HashSet<usize>,
            order: &mut Vec<usize>,
        ) -> Result<(), RegistryError> {
            if done.contains(&i) {
                return Ok(());
            }
            let m = plugins[i].manifest();
            if !visiting.insert(i) {
                return Err(RegistryError::Cycle(m.id));
            }
            for dep in m.depends_on {
                visit(by_id[dep], plugins, by_id, done, visiting, order)?;
            }
            visiting.remove(&i);
            done.insert(i);
            order.push(i);
            Ok(())
        }
        for i in 0..plugins.len() {
            visit(i, &plugins, &by_id, &mut done, &mut visiting, &mut order)?;
        }
        let mut slots: Vec<Option<Box<dyn Plugin>>> = plugins.into_iter().map(Some).collect();
        let plugins = order
            .into_iter()
            .map(|i| slots[i].take().expect("visited once"))
            .collect();
        Ok(Self { plugins })
    }

    /// Plugins in dependency order (dependencies first).
    pub fn plugins(&self) -> &[Box<dyn Plugin>] {
        &self.plugins
    }

    /// Find the plugin and entity definition for an op's entity name.
    pub fn entity(&self, name: &str) -> Option<(&dyn Plugin, &EntityDef)> {
        self.plugins.iter().find_map(|p| {
            p.entities()
                .iter()
                .find(|e| e.name == name)
                .map(|e| (p.as_ref(), e))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake(Manifest, &'static [EntityDef]);
    impl Plugin for Fake {
        fn manifest(&self) -> Manifest {
            self.0
        }
        fn entities(&self) -> &'static [EntityDef] {
            self.1
        }
        fn migrations(&self) -> &'static [Migration] {
            &[]
        }
    }
    fn fake(id: &'static str, deps: &'static [&'static str]) -> Box<dyn Plugin> {
        Box::new(Fake(
            Manifest {
                id,
                name: id,
                version: "0",
                depends_on: deps,
            },
            &[],
        ))
    }
    fn ids(r: &Registry) -> Vec<&'static str> {
        r.plugins().iter().map(|p| p.manifest().id).collect()
    }

    #[test]
    fn orders_dependencies_first() {
        let r = Registry::new(vec![
            fake("sleep", &["screentime"]),
            fake("screentime", &[]),
        ])
        .unwrap();
        assert_eq!(ids(&r), ["screentime", "sleep"]);
    }

    #[test]
    fn rejects_duplicates_missing_deps_and_cycles() {
        assert_eq!(
            Registry::new(vec![fake("a", &[]), fake("a", &[])]).err(),
            Some(RegistryError::Duplicate("a"))
        );
        assert_eq!(
            Registry::new(vec![fake("a", &["zzz"])]).err(),
            Some(RegistryError::MissingDependency {
                plugin: "a",
                missing: "zzz"
            })
        );
        assert!(matches!(
            Registry::new(vec![fake("a", &["b"]), fake("b", &["a"])]).err(),
            Some(RegistryError::Cycle(_))
        ));
    }

    #[test]
    fn entity_names_must_carry_the_plugin_prefix() {
        static E: [EntityDef; 1] = [EntityDef {
            name: "other_thing",
            table: "t",
            columns: &[],
        }];
        let p = Box::new(Fake(
            Manifest {
                id: "body",
                name: "b",
                version: "0",
                depends_on: &[],
            },
            &E,
        ));
        assert!(matches!(
            Registry::new(vec![p]).err(),
            Some(RegistryError::BadEntityName { .. })
        ));
    }
}
