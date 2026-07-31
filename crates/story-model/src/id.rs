//! Newtype identifiers shared across the engine.
//!
//! Each ID is a transparent wrapper over `String` so that it serializes as a
//! bare JSON string (e.g. `"c-namu"`) rather than `{"0":"c-namu"}`. They are
//! intentionally opaque: equality, ordering, and hashing defer to the inner
//! string, and a `Display` impl renders the inner value for diagnostics.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Macro to declare a transparent newtype ID with the canonical derive set and
/// a `Display` impl that renders the inner string.
macro_rules! newtype_id {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(
            Clone,
            Debug,
            Eq,
            PartialEq,
            Ord,
            PartialOrd,
            Hash,
            Serialize,
            Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

newtype_id!(
    /// Identifies a character within a story pack (e.g. `"c-namu"`).
    CharacterId
);
newtype_id!(
    /// Identifies a location within a story pack.
    LocationId
);
newtype_id!(
    /// Identifies a story-defined flag. Carried as a `String` semantically but
    /// kept as a newtype so it cannot be confused with arbitrary strings.
    FlagId
);
newtype_id!(
    /// Identifies a one-shot event that may have occurred.
    EventId
);
newtype_id!(
    /// Identifies an inventory item.
    ItemId
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_transparent_and_display() {
        let c = CharacterId("c-namu".to_string());
        assert_eq!(c.to_string(), "c-namu");
        let l = LocationId("loc-classroom".to_string());
        assert_eq!(format!("{l}"), "loc-classroom");
        assert_eq!(FlagId("f-met".to_string()).to_string(), "f-met");
        assert_eq!(EventId("e-arrival".to_string()).to_string(), "e-arrival");
        assert_eq!(ItemId("i-key".to_string()).to_string(), "i-key");
    }
}
