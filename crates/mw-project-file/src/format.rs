//! How project files are written (design §3, §6): JSON whose outer levels
//! are indented and whose list items each take one line, so that changing
//! one site changes one line; and content hashes.

use std::fmt;
use std::io;

use mw_crystal::Structure;
use serde::{Deserialize, Serialize};
use serde_json::ser::Formatter;
use sha2::Digest;

/// Containers nested this deep or deeper are written on one line, like
/// every container inside a list item.
const COMPACT_DEPTH: usize = 4;

/// Writes `value` as a project file does: deterministic, so the same
/// content always gives the same bytes, and ending in a newline.
pub fn to_json<T: Serialize + ?Sized>(value: &T) -> serde_json::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut serializer = serde_json::Serializer::with_formatter(&mut bytes, Layout::default());
    value.serialize(&mut serializer)?;
    bytes.push(b'\n');
    Ok(bytes)
}

#[derive(Default)]
struct Layout {
    frames: Vec<Frame>,
}

struct Frame {
    compact: bool,
    array: bool,
    empty: bool,
}

impl Layout {
    fn open<W: ?Sized + io::Write>(
        &mut self,
        writer: &mut W,
        array: bool,
        bracket: &[u8],
    ) -> io::Result<()> {
        let compact = self.frames.len() + 1 >= COMPACT_DEPTH
            || self.frames.last().is_some_and(|f| f.compact || f.array);
        self.frames.push(Frame {
            compact,
            array,
            empty: true,
        });
        writer.write_all(bracket)
    }

    fn close<W: ?Sized + io::Write>(&mut self, writer: &mut W, bracket: &[u8]) -> io::Result<()> {
        let frame = self.frames.pop().expect("balanced");
        if !frame.compact && !frame.empty {
            self.newline(writer)?;
        }
        writer.write_all(bracket)
    }

    fn item<W: ?Sized + io::Write>(&mut self, writer: &mut W, first: bool) -> io::Result<()> {
        let compact = self.frames.last().is_some_and(|f| f.compact);
        match (compact, first) {
            (true, true) => Ok(()),
            (true, false) => writer.write_all(b", "),
            (false, first) => {
                if !first {
                    writer.write_all(b",")?;
                }
                let depth = self.frames.len();
                writer.write_all(b"\n")?;
                (0..depth).try_for_each(|_| writer.write_all(b"  "))
            }
        }
    }

    fn newline<W: ?Sized + io::Write>(&self, writer: &mut W) -> io::Result<()> {
        writer.write_all(b"\n")?;
        (0..self.frames.len()).try_for_each(|_| writer.write_all(b"  "))
    }

    fn filled(&mut self) {
        if let Some(frame) = self.frames.last_mut() {
            frame.empty = false;
        }
    }
}

impl Formatter for Layout {
    fn begin_array<W: ?Sized + io::Write>(&mut self, writer: &mut W) -> io::Result<()> {
        self.open(writer, true, b"[")
    }

    fn end_array<W: ?Sized + io::Write>(&mut self, writer: &mut W) -> io::Result<()> {
        self.close(writer, b"]")
    }

    fn begin_array_value<W: ?Sized + io::Write>(
        &mut self,
        writer: &mut W,
        first: bool,
    ) -> io::Result<()> {
        self.item(writer, first)
    }

    fn end_array_value<W: ?Sized + io::Write>(&mut self, _: &mut W) -> io::Result<()> {
        self.filled();
        Ok(())
    }

    fn begin_object<W: ?Sized + io::Write>(&mut self, writer: &mut W) -> io::Result<()> {
        self.open(writer, false, b"{")
    }

    fn end_object<W: ?Sized + io::Write>(&mut self, writer: &mut W) -> io::Result<()> {
        self.close(writer, b"}")
    }

    fn begin_object_key<W: ?Sized + io::Write>(
        &mut self,
        writer: &mut W,
        first: bool,
    ) -> io::Result<()> {
        self.item(writer, first)
    }

    fn begin_object_value<W: ?Sized + io::Write>(&mut self, writer: &mut W) -> io::Result<()> {
        writer.write_all(b": ")
    }

    fn end_object_value<W: ?Sized + io::Write>(&mut self, _: &mut W) -> io::Result<()> {
        self.filled();
        Ok(())
    }
}

/// A SHA-256 digest in lowercase hexadecimal: the name of an imported
/// file's copy, and the fingerprint of a structure's state.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Sha256(String);

impl Sha256 {
    pub fn of(bytes: &[u8]) -> Self {
        let digest = sha2::Sha256::digest(bytes);
        Self(digest.iter().map(|byte| format!("{byte:02x}")).collect())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether `text` is a digest as this type writes it.
    pub(crate) fn is_digest(text: &str) -> bool {
        text.len() == 64
            && text
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    }
}

impl fmt::Display for Sha256 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<Sha256> for String {
    fn from(sha: Sha256) -> Self {
        sha.0
    }
}

impl TryFrom<String> for Sha256 {
    type Error = String;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        if Self::is_digest(&text) {
            Ok(Self(text))
        } else {
            Err(format!("{text} is not a SHA-256 digest"))
        }
    }
}

/// The fingerprint of a structure's state: the SHA-256 of its compact JSON.
/// Equal structures have equal fingerprints within one format version
/// (design §7.1).
pub fn structure_sha256(structure: &Structure) -> Sha256 {
    let json = serde_json::to_vec(structure).expect("structures serialize");
    Sha256::of(&json)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_items_take_one_line() {
        // Keys in sorted order: `json!` keeps insertion order or sorts,
        // depending on serde_json's features in the build.
        let value = serde_json::json!({
            "empty": [],
            "items": [{"id": 1, "position": [0.5, 0.0, 0.25]}, {"id": 2, "position": []}],
            "name": "x",
            "nested": {"cell": {"a": {"value": 1.0}}, "list": ["p", "q"]}
        });
        let text = String::from_utf8(to_json(&value).unwrap()).unwrap();
        assert_eq!(
            text,
            r#"{
  "empty": [],
  "items": [
    {"id": 1, "position": [0.5, 0.0, 0.25]},
    {"id": 2, "position": []}
  ],
  "name": "x",
  "nested": {
    "cell": {
      "a": {"value": 1.0}
    },
    "list": [
      "p",
      "q"
    ]
  }
}
"#
        );
        let read: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(read, value);
    }

    #[test]
    fn digests() {
        assert_eq!(
            Sha256::of(b"abc").as_str(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert!(Sha256::try_from("ABC".to_string()).is_err());
    }
}
