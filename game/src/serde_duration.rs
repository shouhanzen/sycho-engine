use std::time::Duration;

use serde::{Deserialize, Deserializer, Serializer};

pub fn serialize<S>(value: &Duration, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let millis = value.as_millis().min(u64::MAX as u128) as u64;
    serializer.serialize_u64(millis)
}

pub fn deserialize<'de, D>(deserializer: D) -> Result<Duration, D::Error>
where
    D: Deserializer<'de>,
{
    let millis = u64::deserialize(deserializer)?;
    Ok(Duration::from_millis(millis))
}
