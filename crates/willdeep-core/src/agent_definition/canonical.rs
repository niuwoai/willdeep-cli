use anyhow::{Result, bail};
use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde_json::Value;
use std::collections::BTreeMap;

struct Unique;
impl<'de> DeserializeSeed<'de> for Unique {
    type Value = Value;
    fn deserialize<D: de::Deserializer<'de>>(self, d: D) -> std::result::Result<Value, D::Error> {
        d.deserialize_any(self)
    }
}
impl<'de> Visitor<'de> for Unique {
    type Value = Value;
    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("unique-key JSON without numbers")
    }
    fn visit_bool<E: de::Error>(self, v: bool) -> std::result::Result<Value, E> {
        Ok(Value::Bool(v))
    }
    fn visit_str<E: de::Error>(self, v: &str) -> std::result::Result<Value, E> {
        Ok(Value::String(v.into()))
    }
    fn visit_string<E: de::Error>(self, v: String) -> std::result::Result<Value, E> {
        Ok(Value::String(v))
    }
    fn visit_unit<E: de::Error>(self) -> std::result::Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut a: A) -> std::result::Result<Value, A::Error> {
        let mut items = Vec::new();
        while let Some(v) = a.next_element_seed(Unique)? {
            items.push(v);
        }
        Ok(Value::Array(items))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut a: A) -> std::result::Result<Value, A::Error> {
        let mut items = serde_json::Map::new();
        while let Some(k) = a.next_key::<String>()? {
            if items.contains_key(&k) {
                return Err(de::Error::custom("duplicate JSON key"));
            }
            items.insert(k, a.next_value_seed(Unique)?);
        }
        Ok(Value::Object(items))
    }
}

pub(super) fn parse_unique(bytes: &[u8]) -> Result<Value> {
    let mut d = serde_json::Deserializer::from_slice(bytes);
    let value = Unique.deserialize(&mut d)?;
    d.end()?;
    Ok(value)
}

pub(super) fn encode(value: &Value) -> Result<String> {
    Ok(match value {
        Value::Null => "null".into(),
        Value::Bool(v) => v.to_string(),
        Value::String(v) => string(v),
        Value::Array(items) => format!(
            "[{}]",
            items
                .iter()
                .map(encode)
                .collect::<Result<Vec<_>>>()?
                .join(",")
        ),
        Value::Object(items) => {
            let sorted: BTreeMap<_, _> = items.iter().collect();
            format!(
                "{{{}}}",
                sorted
                    .iter()
                    .map(|(k, v)| Ok(format!("{}:{}", string(k), encode(v)?)))
                    .collect::<Result<Vec<_>>>()?
                    .join(",")
            )
        }
        Value::Number(_) => bail!("numbers are not supported in definition v1"),
    })
}

fn string(value: &str) -> String {
    let mut result = String::from("\"");
    for unit in value.encode_utf16() {
        match unit {
            34 => result.push_str("\\\""),
            92 => result.push_str("\\\\"),
            32..=126 => result.push(char::from_u32(u32::from(unit)).unwrap()),
            _ => result.push_str(&format!("\\u{unit:04x}")),
        }
    }
    result.push('"');
    result
}
