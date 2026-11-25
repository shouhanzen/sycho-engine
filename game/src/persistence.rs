use std::fs::{self, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::Serialize;

static NEXT_TEMP_FILE: AtomicU64 = AtomicU64::new(0);

pub(crate) fn save_json(path: &Path, value: &impl Serialize) -> io::Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }

    let sequence = NEXT_TEMP_FILE.fetch_add(1, Ordering::Relaxed);
    let temporary = path.with_extension(format!("{}.{sequence}.tmp", std::process::id()));
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    let result = (|| {
        let mut writer = BufWriter::new(file);
        serde_json::to_writer_pretty(&mut writer, value)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
        writer.flush()?;
        writer.get_ref().sync_all()?;
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
        save_json(&path, &1).unwrap();
        save_json(&path, &2).unwrap();

        let invalid_json = BTreeMap::from([(vec![1], true)]);
        assert!(save_json(&path, &invalid_json).is_err());
        assert_eq!(
            serde_json::from_slice::<u32>(&fs::read(&path).unwrap()).unwrap(),
            2
        );
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
        fs::remove_dir_all(directory).unwrap();
    }
}
