// Copyright (c) 2019-2022 Alibaba Cloud
// Copyright (c) 2019-2022 Ant Group
//
// SPDX-License-Identifier: Apache-2.0
//

use std::convert::From;

use containerd_shim_protos::cgroups::metrics;
use protobuf::Message;

use super::{StatsInfo, StatsInfoValue};

// TODO: trans from agent proto?
impl From<Option<agent::StatsContainerResponse>> for StatsInfo {
    fn from(c_stats: Option<agent::StatsContainerResponse>) -> Self {
        let mut metric = metrics::Metrics::new();
        let stats = match c_stats {
            None => {
                return StatsInfo { value: None };
            }
            Some(stats) => stats,
        };

        if let Some(cg_stats) = stats.cgroup_stats.into_option() {
            if let Some(cpu) = cg_stats.cpu_stats.into_option() {
                // set protobuf cpu stat
                let mut p_cpu = metrics::CPUStat::new();
                if let Some(usage) = cpu.cpu_usage.into_option() {
                    let mut p_usage = metrics::CPUUsage::new();
                    p_usage.set_total(usage.total_usage);
                    p_usage.set_kernel(usage.usage_in_kernelmode);
                    p_usage.set_user(usage.usage_in_usermode);

                    // set protobuf cpu usage
                    p_cpu.set_usage(p_usage);
                }

                if let Some(throttle) = cpu.throttling_data.into_option() {
                    let mut p_throttle = metrics::Throttle::new();
                    p_throttle.set_periods(throttle.periods);
                    p_throttle.set_throttled_time(throttle.throttled_time);
                    p_throttle.set_throttled_periods(throttle.throttled_periods);

                    // set protobuf cpu usage
                    p_cpu.set_throttling(p_throttle);
                }

                metric.set_cpu(p_cpu);
            }

            if let Some(m_stats) = cg_stats.memory_stats.into_option() {
                let mut p_m = metrics::MemoryStat::new();
                p_m.set_cache(m_stats.cache);
                // memory usage
                if let Some(m_data) = m_stats.usage.into_option() {
                    let mut p_m_entry = metrics::MemoryEntry::new();
                    p_m_entry.set_usage(m_data.usage);
                    p_m_entry.set_limit(m_data.limit);
                    p_m_entry.set_failcnt(m_data.failcnt);
                    p_m_entry.set_max(m_data.max_usage);

                    p_m.set_usage(p_m_entry);
                }
                // memory swap_usage
                if let Some(m_data) = m_stats.swap_usage.into_option() {
                    let mut p_m_entry = metrics::MemoryEntry::new();
                    p_m_entry.set_usage(m_data.usage);
                    p_m_entry.set_limit(m_data.limit);
                    p_m_entry.set_failcnt(m_data.failcnt);
                    p_m_entry.set_max(m_data.max_usage);

                    p_m.set_swap(p_m_entry);
                }
                for (k, v) in m_stats.stats {
                    match k.as_str() {
                        "inactive_file" => p_m.set_inactive_file(v),
                        "inactive_anon" => p_m.set_inactive_anon(v),
                        "active_file" => p_m.set_active_file(v),
                        "unevictable" => p_m.set_unevictable(v),
                        _ => (),
                    }
                }
                metric.set_memory(p_m);
            }

            if let Some(pid_stats) = cg_stats.pids_stats.into_option() {
                let mut p_pid = metrics::PidsStat::new();
                p_pid.set_limit(pid_stats.limit);
                p_pid.set_current(pid_stats.current);
                metric.set_pids(p_pid);
            }

            if let Some(blk_stats) = cg_stats.blkio_stats.into_option() {
                let mut p_blk_stats = metrics::BlkIOStat::new();
                p_blk_stats.set_io_service_bytes_recursive(copy_blkio_entry(
                    &blk_stats.io_service_bytes_recursive,
                ));

                metric.set_blkio(p_blk_stats);
            }

            if !cg_stats.hugetlb_stats.is_empty() {
                let mut p_huge = Vec::new();
                for (k, v) in cg_stats.hugetlb_stats {
                    let mut h = metrics::HugetlbStat::new();
                    h.set_pagesize(k);
                    h.set_max(v.max_usage);
                    h.set_usage(v.usage);
                    h.set_failcnt(v.failcnt);
                    p_huge.push(h);
                }
                metric.set_hugetlb(p_huge);
            }
        }

        StatsInfo {
            value: Some(StatsInfoValue {
                type_url: "io.containerd.cgroups.v1.Metrics".to_string(),
                value: metric.write_to_bytes().unwrap(),
            }),
        }
    }
}

fn copy_blkio_entry(entry: &[agent::BlkioStatsEntry]) -> Vec<metrics::BlkIOEntry> {
    let mut p_entry = Vec::new();

    for e in entry.iter() {
        let mut blk = metrics::BlkIOEntry::new();
        blk.set_op(e.op.clone());
        blk.set_value(e.value);
        blk.set_major(e.major);
        blk.set_minor(e.minor);

        p_entry.push(blk);
    }

    p_entry
}

#[cfg(test)]
mod tests {
    use super::*;
    use protobuf::MessageField;
    use protocols::agent as wire;

    #[test]
    fn protobuf_stats_preserve_containerd_accounting() {
        let stats = wire::StatsContainerResponse {
            cgroup_stats: MessageField::some(wire::CgroupStats {
                cpu_stats: MessageField::some(wire::CpuStats {
                    cpu_usage: MessageField::some(wire::CpuUsage {
                        total_usage: 123,
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                memory_stats: MessageField::some(wire::MemoryStats {
                    usage: MessageField::some(wire::MemoryData {
                        usage: 4096,
                        limit: 8192,
                        ..Default::default()
                    }),
                    stats: [("inactive_file".into(), 1024), ("unused-key".into(), 999)].into(),
                    ..Default::default()
                }),
                pids_stats: MessageField::some(wire::PidsStats {
                    current: 3,
                    limit: 100,
                    ..Default::default()
                }),
                blkio_stats: MessageField::some(wire::BlkioStats {
                    io_service_bytes_recursive: vec![wire::BlkioStatsEntry {
                        major: 254,
                        minor: 1,
                        op: "Read".into(),
                        value: 512,
                        ..Default::default()
                    }],
                    ..Default::default()
                }),
                hugetlb_stats: [(
                    "2MB".into(),
                    wire::HugetlbStats {
                        usage: 2048,
                        ..Default::default()
                    },
                )]
                .into(),
                ..Default::default()
            }),
            ..Default::default()
        };
        let info = StatsInfo::from(Some(stats)).value.unwrap();
        assert_eq!(info.type_url, "io.containerd.cgroups.v1.Metrics");
        let result = metrics::Metrics::parse_from_bytes(&info.value).unwrap();
        assert_eq!(result.cpu().usage().total(), 123);
        assert!(result.cpu().usage().per_cpu().is_empty());
        assert_eq!(result.memory().usage().usage(), 4096);
        assert_eq!(result.memory().inactive_file(), 1024);
        assert_eq!(result.pids().current(), 3);
        assert_eq!(result.blkio().io_service_bytes_recursive()[0].value(), 512);
        assert_eq!(result.hugetlb()[0].pagesize(), "2MB");
        assert!(result.network().is_empty());
        assert!(StatsInfo::from(None).value.is_none());
    }
}
