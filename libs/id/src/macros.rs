//! Macros for defining typed ID types.

/// Macro to define a typed ID with a specific prefix.
///
/// This generates a newtype wrapper around ULID with:
/// - A `PREFIX` constant
/// - `new()` to generate a fresh ID
/// - `parse()` to parse from string
/// - `Display` and `FromStr` implementations
/// - `Serialize` and `Deserialize` implementations
/// - `Ord`, `Hash`, and other standard traits
///
/// # Example
///
/// ```ignore
/// define_id!(OrgId, "org");
/// define_id!(AppId, "app");
///
/// let org_id = OrgId::new();
/// let parsed: OrgId = "org_01HV4Z2WQXKJNM8GPQY6VBKC3D".parse()?;
/// ```
#[macro_export]
macro_rules! define_id {
    ($name:ident, $prefix:literal) => {
        /// A typed ID for this resource type.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name($crate::Ulid);

        impl $name {
            /// The prefix for this ID type.
            pub const PREFIX: &'static str = $prefix;

            /// Creates a new ID with a fresh ULID.
            #[must_use]
            pub fn new() -> Self {
                Self($crate::Ulid::new())
            }

            /// Creates an ID from a raw ULID.
            #[must_use]
            pub const fn from_ulid(ulid: $crate::Ulid) -> Self {
                Self(ulid)
            }

            /// Returns the underlying ULID.
            #[must_use]
            pub const fn ulid(&self) -> $crate::Ulid {
                self.0
            }

            /// Returns the timestamp portion of the ULID in milliseconds.
            #[must_use]
            pub fn timestamp_ms(&self) -> u64 {
                self.0.timestamp_ms()
            }

            /// Parses an ID from a string.
            ///
            /// The string must be in the format `{prefix}_{ulid}`.
            pub fn parse(s: &str) -> Result<Self, $crate::IdError> {
                if s.is_empty() {
                    return Err($crate::IdError::Empty);
                }

                let Some((prefix, ulid_str)) = s.split_once('_') else {
                    return Err($crate::IdError::MissingSeparator);
                };

                if prefix != Self::PREFIX {
                    return Err($crate::IdError::InvalidPrefix {
                        expected: Self::PREFIX,
                        actual: prefix.to_string(),
                    });
                }

                let ulid = ulid_str
                    .parse::<$crate::Ulid>()
                    .map_err(|e| $crate::IdError::InvalidUlid(e.to_string()))?;

                Ok(Self(ulid))
            }

            /// Formats the ID as a string.
            #[must_use]
            pub fn to_string(&self) -> String {
                format!("{}_{}", Self::PREFIX, self.0)
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}_{}", Self::PREFIX, self.0)
            }
        }

        impl std::str::FromStr for $name {
            type Err = $crate::IdError;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Self::parse(s)
            }
        }

        impl serde::Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: serde::Serializer,
            {
                serializer.serialize_str(&self.to_string())
            }
        }

        impl<'de> serde::Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                let s = String::deserialize(deserializer)?;
                Self::parse(&s).map_err(serde::de::Error::custom)
            }
        }

        impl AsRef<$crate::Ulid> for $name {
            fn as_ref(&self) -> &$crate::Ulid {
                &self.0
            }
        }
    };
}
