// Copyright (c) 2019-2022 Alibaba Cloud
// Copyright (c) 2019-2022 Ant Group
//
// SPDX-License-Identifier: Apache-2.0
//

pub mod sandbox_persist;
use anyhow::{anyhow, Context, Ok, Result};
use kata_types::config::KATA_PATH;
use serde::de;
use std::{fs::File, io::BufReader};

pub const PERSIST_FILE: &str = "state.json";
use kata_sys_util::validate::verify_id;
use safe_path::scoped_join;

pub fn to_disk<T: serde::Serialize>(value: &T, sid: &str, jailer_path: &str) -> Result<()> {
    verify_id(sid).context("failed to verify sid")?;
    anyhow::ensure!(
        !jailer_path.is_empty(),
        "kata-fc: jailed persistence path is required"
    );
    let mut path = scoped_join(jailer_path, "root")?;
    if path.exists() {
        path.push(PERSIST_FILE);
        let f = File::create(path)
            .context("failed to create the file")
            .context("failed to join the path")?;
        let j = serde_json::to_value(value).context("failed to convert to the json value")?;
        serde_json::to_writer_pretty(f, &j)?;
        return Ok(());
    }
    Err(anyhow!("invalid sid {}", sid))
}

pub fn from_disk<T>(sid: &str) -> Result<T>
where
    T: de::DeserializeOwned,
{
    verify_id(sid).context("failed to verify sid")?;
    let mut path = scoped_join(KATA_PATH, sid)?;
    if path.exists() {
        path.push(PERSIST_FILE);
        let file = File::open(path).context("failed to open the file")?;
        let reader = BufReader::new(file);
        return serde_json::from_reader(reader).map_err(|e| anyhow!(e.to_string()));
    }
    Err(anyhow!("invalid sid {}", sid))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persistence_requires_valid_id_and_jail_and_writes_inside_root() {
        let jail = tempfile::tempdir().unwrap();
        let jail_path = jail.path().to_str().unwrap();
        let data = serde_json::json!({"name":"kata","key":1});
        for sid in ["..3", "../../../3", "a/b/c", ".#cdscd."] {
            assert!(to_disk(&data, sid, jail_path).is_err());
        }
        assert!(to_disk(&data, "sandbox", "").is_err());
        assert!(to_disk(&data, "sandbox", jail_path).is_err());
        std::fs::create_dir(jail.path().join("root")).unwrap();
        to_disk(&data, "sandbox", jail_path).unwrap();
        let file = File::open(jail.path().join("root/state.json")).unwrap();
        let restored: serde_json::Value = serde_json::from_reader(file).unwrap();
        assert_eq!(restored, data);
        assert!(!jail.path().join("state.json").exists());
    }
}
