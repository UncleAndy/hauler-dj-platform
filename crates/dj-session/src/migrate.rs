//! Versioned migrations for the session document (DJP-SET-002, DJP-SET-003).
//!
//! A document says which version it is. Anything older is brought forward step by step, so the set
//! of migrations is a chain rather than a pile of special cases; anything newer is refused by name,
//! because silently dropping fields would be worse than telling the user to update.

use serde_json::Value;

use crate::doc::{SCHEMA, SessionDoc, VERSION};
use crate::store::SessionError;

/// Bring a parsed document up to [`VERSION`].
pub fn bring_forward(raw: Value) -> Result<SessionDoc, SessionError> {
    let version = raw
        .get("version")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let mut current = u32::try_from(version).unwrap_or(u32::MAX);
    if current > VERSION {
        return Err(SessionError::TooNew {
            found: current,
            known: VERSION,
        });
    }
    let mut value = raw;
    while current < VERSION {
        value = match current {
            // v0 kept the device at the top level and had no mixer section.
            0 => from_zero(value)?,
            other => {
                return Err(SessionError::NoRoute {
                    from: other,
                    to: VERSION,
                });
            }
        };
        current += 1;
    }
    let mut document: SessionDoc = serde_json::from_value(value).map_err(SessionError::Parse)?;
    // A document from the same version but a different schema name is not ours.
    if document.schema != SCHEMA {
        return Err(SessionError::ForeignSchema(document.schema));
    }
    document.version = VERSION;
    document.schema = SCHEMA.to_owned();
    Ok(document)
}

/// v0 → v1: `audio_device` moves into `audio.device`, and a missing mixer section gets its defaults.
fn from_zero(mut value: Value) -> Result<Value, SessionError> {
    let object = value.as_object_mut().ok_or_else(|| {
        SessionError::Parse(serde_json::Error::io(std::io::Error::other(
            "the document is not an object",
        )))
    })?;
    let device = object.remove("audio_device");
    let audio = object
        .entry("audio")
        .or_insert_with(|| Value::Object(serde_json::Map::new()));
    if let (Some(Value::String(name)), Some(audio)) = (device, audio.as_object_mut()) {
        audio.entry("device").or_insert(Value::String(name));
    }
    object
        .entry("mixer")
        .or_insert_with(|| Value::Object(serde_json::Map::new()));
    object.insert("version".to_owned(), Value::from(1));
    // The shape changed, so the name changes with it: the reader checks the name it knows.
    object.insert("schema".to_owned(), Value::from(SCHEMA));
    Ok(value)
}
