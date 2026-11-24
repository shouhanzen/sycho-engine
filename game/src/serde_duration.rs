use std::time::Duration;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub fn serialize<S>(value: &Duration, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    value.serialize(serializer)
}

pub fn deserialize<'de, D>(deserializer: D) -> Result<Duration, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum StoredDuration {
        Milliseconds(u64),
        Exact(Duration),
    }

    Ok(match StoredDuration::deserialize(deserializer)? {
        StoredDuration::Milliseconds(millis) => Duration::from_millis(millis),
        StoredDuration::Exact(duration) => duration,
    })
}
