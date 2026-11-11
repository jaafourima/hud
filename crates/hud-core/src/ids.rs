//! Typed identifiers. UUIDv7 so ids sort roughly by creation time.

use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

macro_rules! id_type {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(Uuid);

        impl $name {
            pub fn new() -> Self {
                Self(Uuid::now_v7())
            }
        }
        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }
        impl FromStr for $name {
            type Err = uuid::Error;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                s.parse().map(Self)
            }
        }
    };
}

id_type!(/// Identifies one synced record (a weight entry, a workout, ...).
    RecordId);
id_type!(/// Identifies one installed client (a phone, a desktop, ...).
    DeviceId);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrips_through_string_and_json() {
        let id = RecordId::new();
        assert_eq!(id.to_string().parse::<RecordId>().unwrap(), id);
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(serde_json::from_str::<RecordId>(&json).unwrap(), id);
    }

    #[test]
    fn ids_are_unique() {
        assert_ne!(RecordId::new(), RecordId::new());
    }
}
