//! `RUSTGIS_MEMORY_BYTES` stands in for the computer's memory, so the limits `gp.env` reports
//! follow it (`rustgis_data::memory::physical`).

use serde_json::{Value, json};
use std::process::Command;

/// `gp.env`'s `limits` from a fresh CLI process, with the override set to `memory` or removed.
fn limits(memory: Option<&str>) -> Value {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rustgis-cli"));
    cmd.args(["run", "--no-plugins", "--cmd", "gp.env"]).env_remove("RUSTGIS_MEMORY_BYTES");
    if let Some(m) = memory {
        cmd.env("RUSTGIS_MEMORY_BYTES", m);
    }
    let out = cmd.output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let env: Value = serde_json::from_slice(&out.stdout).unwrap();
    env["limits"].clone()
}

#[test]
fn the_memory_override_sets_the_limits() {
    // 8 GiB: a quarter of it, 2 GiB, for the undo history.
    assert_eq!(limits(Some("8589934592"))["undoBytes"], json!(2_u64 << 30));
    // 1 GiB: below every floor.
    assert_eq!(limits(Some("1073741824"))["undoBytes"], json!(256_u64 << 20));
    // A value that isn't a positive number of bytes is ignored.
    assert_eq!(limits(Some("8 GiB")), limits(None));
}
