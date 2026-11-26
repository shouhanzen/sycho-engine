use std::fs::{self, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::Serialize;

static NEXT_TEMP_FILE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy)]
pub struct JsonWriteOptions {
    pub pretty: bool,
    pub sync_file: bool,
}

impl JsonWriteOptions {
    pub const COMPACT: Self = Self {
        pretty: false,
        sync_file: true,
    };
    pub const PRETTY: Self = Self {
        pretty: true,
        sync_file: true,
    };
}

/// Write JSON to a sibling file and atomically replace the destination.
/// Callers can sync the parent directory when the rename must survive a crash.
pub fn write_json_atomic(
    path: &Path,
    value: &impl Serialize,
    options: JsonWriteOptions,
) -> io::Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }

    let (temporary, file) = loop {
        let sequence = NEXT_TEMP_FILE.fetch_add(1, Ordering::Relaxed);
        let temporary = path.with_extension(format!("{}.{sequence}.tmp", std::process::id()));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
            Ok(file) => break (temporary, file),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(err),
        }
    };
    let result = (|| {
        let mut writer = BufWriter::new(file);
        if options.pretty {
            serde_json::to_writer_pretty(&mut writer, value)?;
        } else {
            serde_json::to_writer(&mut writer, value)?;
        }
        writer.flush()?;
        if options.sync_file {
            writer.get_ref().sync_all()?;
        }
        drop(writer);
        fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn save_replaces_complete_json_and_preserves_it_when_serialization_fails() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory =
            std::env::temp_dir().join(format!("sycho-json-save-{}-{nanos}", std::process::id()));
        let path = directory.join("settings.json");
        write_json_atomic(&path, &1, JsonWriteOptions::COMPACT).unwrap();
        write_json_atomic(&path, &2, JsonWriteOptions::PRETTY).unwrap();

        let invalid_json = BTreeMap::from([(vec![1], true)]);
        assert!(write_json_atomic(&path, &invalid_json, JsonWriteOptions::PRETTY).is_err());
        assert_eq!(
            serde_json::from_slice::<u32>(&fs::read(&path).unwrap()).unwrap(),
            2
        );
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
        fs::remove_dir_all(directory).unwrap();
    }
}
