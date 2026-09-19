// Copyright (c) 2020 Ant Group
//
// SPDX-License-Identifier: Apache-2.0
//

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use futures::StreamExt as _;
use inotify::{Inotify, WatchMask};
use tokio::sync::mpsc::{channel, Receiver};

// Convenience function to obtain the scope logger.
fn sl() -> slog::Logger {
    slog_scope::logger().new(o!("subsystem" => "cgroups_notifier"))
}

// get_value_from_cgroup parse cgroup file with `Flat keyed`
// and get the value of `key`.
// Flat keyed file format:
//   KEY0 VAL0\n
//   KEY1 VAL1\n
fn get_value_from_cgroup(path: &Path, key: &str) -> Result<i64> {
    let content = fs::read_to_string(path)?;
    info!(
        sl(),
        "get_value_from_cgroup file: {:?}, content: {}", &path, &content
    );

    for line in content.lines() {
        let arr: Vec<&str> = line.split(' ').collect();
        if arr.len() == 2 && arr[0] == key {
            let r = arr[1].parse::<i64>()?;
            return Ok(r);
        }
    }
    Ok(0)
}

// notify_on_oom returns channel on which you can expect event about OOM,
// if process died without OOM this channel will be closed.
pub async fn notify_oom(containere_id: &str, cg_dir: String) -> Result<Receiver<String>> {
    let event_control_path = Path::new(&cg_dir).join("memory.events");
    let cgroup_event_control_path = Path::new(&cg_dir).join("cgroup.events");
    info!(
        sl(),
        "notify_oom event_control_path: {:?}", &event_control_path
    );
    info!(
        sl(),
        "notify_oom cgroup_event_control_path: {:?}", &cgroup_event_control_path
    );

    let mut inotify = Inotify::init().context("Failed to initialize inotify")?;

    // watching oom kill
    let ev_wd = inotify
        .add_watch(&event_control_path, WatchMask::MODIFY)
        .context(format!("failed to add watch for {:?}", &event_control_path))?;

    // Because no `unix.IN_DELETE|unix.IN_DELETE_SELF` event for cgroup file system, so watching all process exited
    let cg_wd = inotify
        .add_watch(&cgroup_event_control_path, WatchMask::MODIFY)
        .context(format!(
            "failed to add watch for {:?}",
            &cgroup_event_control_path
        ))?;

    info!(sl(), "ev_wd: {:?}", ev_wd);
    info!(sl(), "cg_wd: {:?}", cg_wd);

    let (sender, receiver) = channel(100);
    let containere_id = containere_id.to_string();

    tokio::spawn(async move {
        let mut buffer = [0; 32];
        let mut stream = inotify
            .event_stream(&mut buffer)
            .expect("create inotify event stream failed");

        while let Some(event_or_error) = stream.next().await {
            let event = event_or_error.unwrap();
            info!(
                sl(),
                "container[{}] get event for container: {:?}", &containere_id, &event
            );
            info!(sl(), "event.wd: {:?}", event.wd);

            if event.wd == ev_wd {
                let oom = get_value_from_cgroup(&event_control_path, "oom_kill");
                if oom.unwrap_or(0) > 0 {
                    let _ = sender.send(containere_id.clone()).await.map_err(|e| {
                        error!(sl(), "send containere_id failed, error: {:?}", e);
                    });
                    return;
                }
            } else if event.wd == cg_wd {
                let pids = get_value_from_cgroup(&cgroup_event_control_path, "populated");
                if pids.unwrap_or(-1) == 0 {
                    return;
                }
            }

            // Stop watching a destroyed cgroup.
            if !Path::new(&event_control_path).exists() {
                return;
            }
        }
    });

    Ok(receiver)
}
