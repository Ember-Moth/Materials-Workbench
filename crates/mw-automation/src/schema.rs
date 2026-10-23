//! JSON Schemas for capability inputs and outputs, built from the Rust
//! types that (de)serialize them, so the two cannot drift apart.
//!
//! The schemas stay in the strict subset of `docs/design/ai-integration.md`
//! §6.2.6: every object lists all its properties as required and allows no
//! others, an optional value is one type plus `null`, and ranges and units
//! go in descriptions, checked by the handlers.

use mw_crystal::{AtomRef, Axis, CellParam, SiteId};
use serde_json::{Map, Value, json};

/// A type with a JSON Schema for its serde form.
pub trait Schema {
    fn schema() -> Value;
}

macro_rules! scalar {
    ($($ty:ty => $name:literal),* $(,)?) => {
        $(impl Schema for $ty {
            fn schema() -> Value {
                json!({ "type": $name })
            }
        })*
    };
}

scalar!(
    f64 => "number",
    u16 => "integer",
    u32 => "integer",
    u64 => "integer",
    usize => "integer",
    bool => "boolean",
    String => "string",
);

impl<T: Schema> Schema for Vec<T> {
    fn schema() -> Value {
        json!({ "type": "array", "items": T::schema() })
    }
}

impl<T: Schema, const N: usize> Schema for [T; N] {
    fn schema() -> Value {
        json!({ "type": "array", "items": T::schema() })
    }
}

/// One type plus `null`. Only for outputs: every nullable input counts
/// against the request's limit on union types.
impl<T: Schema> Schema for Option<T> {
    fn schema() -> Value {
        let mut schema = T::schema();
        if let Some(name) = schema.get("type").and_then(Value::as_str) {
            schema["type"] = json!([name, "null"]);
        }
        if let Some(values) = schema.get_mut("enum").and_then(Value::as_array_mut) {
            values.push(Value::Null);
        }
        schema
    }
}

/// A list with at least one item.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "Vec<T>")]
pub struct NonEmpty<T>(pub Vec<T>);

impl<T> TryFrom<Vec<T>> for NonEmpty<T> {
    type Error = &'static str;

    fn try_from(items: Vec<T>) -> Result<Self, Self::Error> {
        if items.is_empty() {
            Err("expected at least one item")
        } else {
            Ok(Self(items))
        }
    }
}

impl<T: Schema> Schema for NonEmpty<T> {
    fn schema() -> Value {
        json!({ "type": "array", "items": T::schema(), "minItems": 1 })
    }
}

impl Schema for SiteId {
    fn schema() -> Value {
        json!({ "type": "integer" })
    }
}

impl Schema for AtomRef {
    fn schema() -> Value {
        object(&[
            (
                "site",
                SiteId::schema(),
                "Id of the site the atom belongs to.",
            ),
            (
                "member",
                usize::schema(),
                "Which of the site's equivalent atoms in the unit cell, from 0. Member 0 \
                 is the site's own position wrapped into the cell.",
            ),
            (
                "cell",
                <[i64; 3]>::schema(),
                "Lattice translation [na, nb, nc] of the atom from the unit cell, three \
                 integers.",
            ),
        ])
    }
}

impl Schema for i64 {
    fn schema() -> Value {
        json!({ "type": "integer" })
    }
}

impl Schema for CellParam {
    fn schema() -> Value {
        json!({
            "type": "string",
            "enum": ["a", "b", "c", "alpha", "beta", "gamma"],
        })
    }
}

impl Schema for Axis {
    fn schema() -> Value {
        json!({ "type": "string", "enum": ["x", "y", "z"] })
    }
}

/// An object with the given properties, all required, and no others. Each
/// description is a doc comment's lines, joined.
pub fn object(fields: &[(&str, Value, &str)]) -> Value {
    let mut properties = Map::new();
    for (name, schema, description) in fields {
        let mut schema = schema.clone();
        let description = description
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        if !description.is_empty() {
            schema["description"] = json!(description);
        }
        properties.insert((*name).to_string(), schema);
    }
    let required: Vec<&str> = fields.iter().map(|(name, ..)| *name).collect();
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
}

/// Declares a struct for a capability's input or output and its schema,
/// from one field list. Field doc comments become the descriptions, which
/// the model reads: state units and limits there.
macro_rules! schema_struct {
    (
        $(#[$meta:meta])*
        pub struct $name:ident {
            $(
                $(#[doc = $doc:literal])*
                pub $field:ident : $ty:ty
            ),* $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        pub struct $name {
            $(
                $(#[doc = $doc])*
                pub $field: $ty,
            )*
        }

        impl $crate::schema::Schema for $name {
            fn schema() -> serde_json::Value {
                $crate::schema::object(&[$(
                    (
                        stringify!($field),
                        <$ty as $crate::schema::Schema>::schema(),
                        concat!($($doc, "\n",)* ""),
                    ),
                )*])
            }
        }
    };
}

/// Declares a string enum for inputs or outputs and its schema.
macro_rules! string_enum {
    (
        $(#[$meta:meta])*
        pub enum $name:ident {
            $( $(#[doc = $doc:literal])* $variant:ident => $text:literal ),* $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
        pub enum $name {
            $( $(#[doc = $doc])* #[serde(rename = $text)] $variant, )*
        }

        impl $crate::schema::Schema for $name {
            fn schema() -> serde_json::Value {
                serde_json::json!({ "type": "string", "enum": [$($text),*] })
            }
        }
    };
}

pub(crate) use {schema_struct, string_enum};

#[cfg(test)]
mod tests {
    use super::*;

    schema_struct! {
        /// An example.
        pub struct Example {
            /// Å, positive.
            /// Second line.
            pub length: f64,
            pub atoms: NonEmpty<AtomRef>,
            pub note: Option<String>,
        }
    }

    #[test]
    fn objects_require_every_field() {
        let schema = Example::schema();
        assert_eq!(schema["required"], json!(["length", "atoms", "note"]));
        assert_eq!(schema["additionalProperties"], json!(false));
        assert_eq!(
            schema["properties"]["length"],
            json!({ "type": "number", "description": "Å, positive. Second line." })
        );
        assert_eq!(schema["properties"]["atoms"]["minItems"], json!(1));
        assert_eq!(
            schema["properties"]["note"]["type"],
            json!(["string", "null"])
        );
    }

    #[test]
    fn deserializing_follows_the_schema() {
        let atom = json!({ "site": 1, "member": 0, "cell": [0, 0, 0] });
        let ok: Example =
            serde_json::from_value(json!({ "length": 1.5, "atoms": [atom], "note": null }))
                .unwrap();
        assert_eq!(ok.atoms.0.len(), 1);
        let empty = json!({ "length": 1.5, "atoms": [], "note": null });
        assert!(serde_json::from_value::<Example>(empty).is_err());
        let extra = json!({ "length": 1.5, "atoms": [atom], "note": null, "x": 1 });
        assert!(serde_json::from_value::<Example>(extra).is_err());
    }
}
