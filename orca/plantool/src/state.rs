use std::collections::HashMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::plans::Task;

const DEFAULT_LEASE_MINUTES: i64 = 30;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claim {
    pub task_id: String,
    pub owner: String,
    pub claimed_at: DateTime<Utc>,
    pub lease_until: DateTime<Utc>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ClaimStore {
    pub claims: HashMap<String, Claim>,
}

impl ClaimStore {
    pub fn load(root: &Path) -> Result<Self> {
        let path = claims_path(root);
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = fs::read_to_string(&path)
            .with_context(|| format!("Failed to read {}", path.display()))?;
        let store: Self = serde_json::from_str(&text)
            .with_context(|| format!("Failed to parse {}", path.display()))?;
        Ok(store)
    }

    pub fn save(&self, root: &Path) -> Result<()> {
        let dir = state_dir(root);
        fs::create_dir_all(&dir).with_context(|| format!("Failed to create {}", dir.display()))?;
        let path = claims_path(root);
        let text = serde_json::to_string_pretty(self)?;
        fs::write(&path, text).with_context(|| format!("Failed to write {}", path.display()))?;
        Ok(())
    }

    pub fn active_claim<'a>(&'a self, task_id: &str, now: DateTime<Utc>) -> Option<&'a Claim> {
        self.claims.get(task_id).filter(|c| c.lease_until > now)
    }

    pub fn claim(&mut self, task_id: &str, owner: &str, now: DateTime<Utc>) -> Result<()> {
        if let Some(existing) = self.claims.get(task_id) {
            if existing.lease_until > now && existing.owner != owner {
                bail!(
                    "Task {} is already claimed by {} until {}",
                    task_id,
                    existing.owner,
                    existing.lease_until
                );
            }
        }
        let claim = Claim {
            task_id: task_id.to_string(),
            owner: owner.to_string(),
            claimed_at: now,
            lease_until: now + Duration::minutes(DEFAULT_LEASE_MINUTES),
        };
        self.claims.insert(task_id.to_string(), claim);
        Ok(())
    }

    pub fn release(&mut self, task_id: &str) {
        self.claims.remove(task_id);
    }
}

pub fn mark_task_done(task: &Task, note: Option<&str>) -> Result<()> {
    let path = &task.plan_path;
    let text =
        fs::read_to_string(path).with_context(|| format!("Failed to read {}", path.display()))?;
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let mut lines: Vec<String> = text.lines().map(|s| s.to_string()).collect();

    let Some(line) = lines.get(task.line_index).cloned() else {
        bail!(
            "Task {} references missing line index {} in {}",
            task.id,
            task.line_index,
            path.display()
        );
    };

    let trimmed = line.trim_start();
    let indent_len = line.len() - trimmed.len();
    let indent = " ".repeat(indent_len);
    let updated = if let Some(rest) = trimmed.strip_prefix("- [ ]") {
        format!("{}- [x]{}", indent, rest)
    } else if trimmed.starts_with("- [x]") || trimmed.starts_with("- [X]") {
        line
    } else {
        bail!(
            "Task {} does not point to an unchecked checklist item in {}",
            task.id,
            path.display()
        );
    };
    lines[task.line_index] = updated;

    if let Some(n) = note {
        if !n.trim().is_empty() {
            lines.push(String::new());
            lines.push(format!("Completion Note: {}", n.trim()));
        }
    }

    let mut out = lines.join(newline);
    if text.ends_with('\n') || text.ends_with("\r\n") {
        out.push_str(newline);
    }
    fs::write(path, out).with_context(|| format!("Failed to write {}", path.display()))?;
    Ok(())
}

fn state_dir(root: &Path) -> std::path::PathBuf {
    root.join("orca").join("plantool").join("state")
}

fn claims_path(root: &Path) -> std::path::PathBuf {
    state_dir(root).join("claims.json")
}
