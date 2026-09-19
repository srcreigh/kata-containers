// Copyright (c) 2019, 2020 Ant Group
//
// SPDX-License-Identifier: Apache-2.0
//

use crate::cgroups_rs as cgroups;
use cgroups::blkio::BlkIoController;
use cgroups::cpu::CpuController;
use cgroups::cpuset::CpuSetController;
use cgroups::devices::DevicePermissions;
use cgroups::devices::DeviceType;
use cgroups::freezer::{FreezerController, FreezerState};
use cgroups::hugetlb::HugeTlbController;
use cgroups::memory::MemController;
use cgroups::pid::PidController;
use cgroups::{
    BlkIoDeviceResource, BlkIoDeviceThrottleResource, Cgroup, CgroupPid, Controller,
    DeviceResource, HugePageResource, MaxValue,
};

use crate::cgroups::{rule_for_all_devices, Manager as CgroupManager};
use crate::container::DEFAULT_DEVICES;
use anyhow::{anyhow, Context, Result};
use libc::pid_t;
use oci::{
    LinuxBlockIo, LinuxCpu, LinuxDevice, LinuxDeviceCgroup, LinuxDeviceCgroupBuilder,
    LinuxHugepageLimit, LinuxMemory, LinuxPids, LinuxResources, Spec,
};
use oci_spec::runtime as oci;

use protobuf::MessageField;
use protocols::agent::{
    BlkioStats, BlkioStatsEntry, CgroupStats, CpuStats, CpuUsage, HugetlbStats, MemoryData,
    MemoryStats, PidsStats, ThrottlingData,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use super::DevicesCgroupInfo;

const INIT_SUBCGROUP: &str = "/init/";

// Convenience function to obtain the scope logger.
fn sl() -> slog::Logger {
    slog_scope::logger().new(o!("subsystem" => "cgroups"))
}

macro_rules! get_controller_or_return_singular_none {
    ($cg:ident) => {
        match $cg.controller_of() {
            Some(c) => c,
            None => return MessageField::none(),
        }
    };
}

#[derive(Debug, Clone)]
pub struct Manager {
    pub cpath: String,
    cgroup: cgroups::Cgroup,
    pod_cgroup: Option<cgroups::Cgroup>,
    devcg_allowed_all: bool,
}

// set_resource is used to set reources by cgroup controller.
macro_rules! set_resource {
    ($cont:ident, $func:ident, $res:ident, $field:ident) => {
        let resource_value = $res.$field().unwrap_or(0);
        if resource_value != 0 {
            $cont.$func(resource_value)?;
        }
    };
}

impl CgroupManager for Manager {
    fn apply(&self, pid: pid_t) -> Result<()> {
        let cgroup_pid = CgroupPid::from(pid as u64);
        if cgroup_has_init_subcgroup(&self.cpath) {
            let cpath = self.cgroup_path_with_subcgroup(INIT_SUBCGROUP);
            load_cgroup(Box::new(cgroups::hierarchies::V2::new()), &cpath)
                .add_task_by_tgid(cgroup_pid)
                .with_context(|| format!("add task {} to cgroup {}", pid, cpath))?;
        } else {
            self.cgroup
                .add_task_by_tgid(cgroup_pid)
                .with_context(|| format!("add task {} to cgroup {}", pid, self.cpath))?;
        }
        Ok(())
    }

    fn set(&self, r: &LinuxResources) -> Result<()> {
        info!(
            sl(),
            "cgroup manager set resources for container. Resources input {:?}", r
        );

        validate_resources(r)?;

        let res = &mut cgroups::Resources::default();
        let pod_res = &mut cgroups::Resources::default();

        // set cpuset and cpu reources
        if let Some(cpu) = &r.cpu() {
            set_cpu_resources(&self.cgroup, cpu)?;
        }

        // set memory resources
        if let Some(memory) = &r.memory() {
            set_memory_resources(&self.cgroup, memory)?;
        }

        // set pids resources
        if let Some(pids_resources) = &r.pids() {
            set_pids_resources(&self.cgroup, pids_resources)?;
        }

        // set block_io resources
        if let Some(blkio) = &r.block_io() {
            set_block_io_resources(&self.cgroup, blkio, res);
        }

        // set hugepages resources
        if let Some(hugepage_limits) = r.hugepage_limits() {
            set_hugepages_resources(&self.cgroup, hugepage_limits, res);
        }

        // set devices resources
        if !self.devcg_allowed_all {
            if let Some(devices) = r.devices() {
                set_devices_resources(&self.cgroup, devices, res, pod_res);
            }
        }
        debug!(
            sl(),
            "Resources after processed, pod_res = {:?}, res = {:?}", pod_res, res
        );

        // apply resources
        if let Some(pod_cg) = self.pod_cgroup.as_ref() {
            pod_cg.apply(pod_res)?;
        }
        self.cgroup.apply(res)?;

        Ok(())
    }

    fn get_stats(&self) -> Result<CgroupStats> {
        // CpuStats
        let cpu_usage = get_cpu_usage_stats(&self.cgroup);

        let throttling_data = get_cpu_stats(&self.cgroup);

        let cpu_stats = MessageField::some(CpuStats {
            cpu_usage,
            throttling_data,
            ..Default::default()
        });

        // Memorystats
        let memory_stats = get_memory_stats(&self.cgroup);

        // PidsStats
        let pids_stats = get_pids_stats(&self.cgroup);

        // BlkioStats
        // note that virtiofs has no blkio stats
        let blkio_stats = get_blkio_stats(&self.cgroup);

        // HugetlbStats
        let hugetlb_stats = get_hugetlb_stats(&self.cgroup);

        Ok(CgroupStats {
            cpu_stats,
            memory_stats,
            pids_stats,
            blkio_stats,
            hugetlb_stats,
            ..Default::default()
        })
    }

    fn freeze(&self, state: FreezerState) -> Result<()> {
        let freezer_controller: &FreezerController = self.cgroup.controller_of().unwrap();
        match state {
            FreezerState::Thawed => {
                freezer_controller.thaw()?;
            }
            FreezerState::Frozen => {
                freezer_controller.freeze()?;
            }
            _ => {
                return Err(anyhow!("Invalid FreezerState"));
            }
        }

        Ok(())
    }

    fn destroy(&self) -> Result<()> {
        if let Err(err) = self.cgroup.delete() {
            warn!(
                sl(),
                "Failed to delete cgroup {}: {}",
                self.cgroup.path(),
                err
            );
        }
        Ok(())
    }

    fn get_pids(&self) -> Result<Vec<pid_t>> {
        let mem_controller: &MemController = self.cgroup.controller_of().unwrap();
        let pids = mem_controller.tasks();
        let result = pids.iter().map(|x| x.pid as i32).collect::<Vec<i32>>();

        Ok(result)
    }

    fn get_cgroup_path(&self) -> Result<String> {
        Ok(cgroup_path_under_root("/sys/fs/cgroup", &self.cpath)
            .display()
            .to_string())
    }

    fn name(&self) -> &str {
        "cgroupfs"
    }
}

/// Reject controls which have no cgroup-v2 implementation before changing resources.
pub fn validate_resources(resources: &LinuxResources) -> Result<()> {
    if let Some(network) = resources.network() {
        if network.class_id().unwrap_or(0) != 0
            || network.priorities().as_ref().is_some_and(|p| !p.is_empty())
        {
            return Err(anyhow!(
                "kata-fc: cgroup v1 network controls are unsupported"
            ));
        }
    }
    if let Some(cpu) = resources.cpu() {
        if cpu.realtime_period().unwrap_or(0) != 0 || cpu.realtime_runtime().unwrap_or(0) != 0 {
            return Err(anyhow!(
                "kata-fc: cgroup v1 realtime CPU controls are unsupported"
            ));
        }
    }
    if let Some(memory) = resources.memory() {
        if memory.kernel().unwrap_or(0) != 0
            || memory.kernel_tcp().unwrap_or(0) != 0
            || memory.swappiness().is_some()
            || memory.disable_oom_killer().unwrap_or(false)
        {
            return Err(anyhow!(
                "kata-fc: cgroup v1 memory controls are unsupported"
            ));
        }
        convert_memory_swap_to_v2_value(memory.swap().unwrap_or(0), memory.limit().unwrap_or(0))?;
    }
    if let Some(block) = resources.block_io() {
        if block.leaf_weight().unwrap_or(0) != 0
            || block.weight_device().as_ref().is_some_and(|devices| {
                devices
                    .iter()
                    .any(|device| device.leaf_weight().unwrap_or(0) != 0)
            })
        {
            return Err(anyhow!(
                "kata-fc: cgroup v1 block leaf weights are unsupported"
            ));
        }
    }
    Ok(())
}

fn set_devices_resources(
    _cg: &cgroups::Cgroup,
    device_resources: &[LinuxDeviceCgroup],
    res: &mut cgroups::Resources,
    pod_res: &mut cgroups::Resources,
) {
    info!(sl(), "cgroup manager set devices");
    let mut devices = vec![];

    for d in device_resources.iter() {
        if rule_for_all_devices(d) {
            continue;
        }
        if let Some(dev) = linux_device_cgroup_to_device_resource(d) {
            devices.push(dev);
        }
    }

    pod_res.devices.devices = devices.clone();
    res.devices.devices = devices;
}

fn set_hugepages_resources(
    cg: &cgroups::Cgroup,
    hugepage_limits: &[LinuxHugepageLimit],
    res: &mut cgroups::Resources,
) {
    info!(sl(), "cgroup manager set hugepage");
    let mut limits = vec![];
    let hugetlb_controller = cg.controller_of::<HugeTlbController>();

    for l in hugepage_limits.iter() {
        if hugetlb_controller.is_some() && hugetlb_controller.unwrap().size_supported(l.page_size())
        {
            let hr = HugePageResource {
                size: l.page_size().clone(),
                limit: l.limit() as u64,
            };
            limits.push(hr);
        } else {
            warn!(
                sl(),
                "{} page size support cannot be verified, dropping requested limit",
                l.page_size()
            );
        }
    }
    res.hugepages.limits = limits;
}

fn set_block_io_resources(
    _cg: &cgroups::Cgroup,
    blkio: &LinuxBlockIo,
    res: &mut cgroups::Resources,
) {
    info!(sl(), "cgroup manager set block io");

    res.blkio.weight = blkio.weight();

    let mut blk_device_resources = vec![];
    let default_weight_device = vec![];
    let weight_device = blkio
        .weight_device()
        .as_ref()
        .unwrap_or(&default_weight_device);
    for d in weight_device.iter() {
        let dr = BlkIoDeviceResource {
            major: d.major() as u64,
            minor: d.minor() as u64,
            weight: blkio.weight(),
            leaf_weight: None,
        };
        blk_device_resources.push(dr);
    }
    res.blkio.weight_device = blk_device_resources;

    res.blkio.throttle_read_bps_device = build_blk_io_device_throttle_resource(
        blkio.throttle_read_bps_device().as_ref().unwrap_or(&vec![]),
    );
    res.blkio.throttle_write_bps_device = build_blk_io_device_throttle_resource(
        blkio
            .throttle_write_bps_device()
            .as_ref()
            .unwrap_or(&vec![]),
    );
    res.blkio.throttle_read_iops_device = build_blk_io_device_throttle_resource(
        blkio
            .throttle_read_iops_device()
            .as_ref()
            .unwrap_or(&vec![]),
    );
    res.blkio.throttle_write_iops_device = build_blk_io_device_throttle_resource(
        blkio
            .throttle_write_iops_device()
            .as_ref()
            .unwrap_or(&vec![]),
    );
}

fn set_cpu_resources(cg: &cgroups::Cgroup, cpu: &LinuxCpu) -> Result<()> {
    info!(sl(), "cgroup manager set cpu");

    let cpuset_controller: &CpuSetController = cg.controller_of().unwrap();

    if let Some(cpus) = cpu.cpus() {
        if let Err(e) = cpuset_controller.set_cpus(cpus) {
            warn!(sl(), "write cpuset failed: {:?}", e);
        }
    }

    if let Some(mems) = cpu.mems() {
        cpuset_controller.set_mems(mems)?;
    }

    let cpu_controller: &CpuController = cg.controller_of().unwrap();

    if let Some(shares) = cpu.shares() {
        let shares = convert_shares_to_v2_value(shares);
        if shares != 0 {
            cpu_controller.set_shares(shares)?;
        }
    }

    set_resource!(cpu_controller, set_cfs_quota, cpu, quota);
    set_resource!(cpu_controller, set_cfs_period, cpu, period);

    Ok(())
}

fn set_memory_resources(cg: &cgroups::Cgroup, memory: &LinuxMemory) -> Result<()> {
    info!(sl(), "cgroup manager set memory");
    let mem_controller: &MemController = cg.controller_of().unwrap();

    // OCI expresses memory+swap together; v2 limits swap separately.
    let swap =
        convert_memory_swap_to_v2_value(memory.swap().unwrap_or(0), memory.limit().unwrap_or(0))?;
    set_resource!(mem_controller, set_limit, memory, limit);
    if swap != 0 || memory.swap().unwrap_or(0) > 0 {
        mem_controller.set_memswap_limit(swap)?;
    }
    set_resource!(mem_controller, set_soft_limit, memory, reservation);
    Ok(())
}

fn set_pids_resources(cg: &cgroups::Cgroup, pids: &LinuxPids) -> Result<()> {
    info!(sl(), "cgroup manager set pids");
    let pid_controller: &PidController = cg.controller_of().unwrap();
    let v = if pids.limit() > 0 {
        MaxValue::Value(pids.limit())
    } else {
        MaxValue::Max
    };
    pid_controller
        .set_pid_max(v)
        .context("failed to set pids resources")
}

fn build_blk_io_device_throttle_resource(
    input: &[oci::LinuxThrottleDevice],
) -> Vec<BlkIoDeviceThrottleResource> {
    let mut blk_io_device_throttle_resources = vec![];
    for d in input.iter() {
        let tr = BlkIoDeviceThrottleResource {
            major: d.major() as u64,
            minor: d.minor() as u64,
            rate: d.rate(),
        };
        blk_io_device_throttle_resources.push(tr);
    }

    blk_io_device_throttle_resources
}

fn linux_device_cgroup_to_device_resource(d: &LinuxDeviceCgroup) -> Option<DeviceResource> {
    let dev_type = DeviceType::from_char(d.typ().unwrap_or_default().as_str().chars().next())?;

    let mut permissions: Vec<DevicePermissions> = vec![];
    for p in d
        .access()
        .as_ref()
        .unwrap_or(&"".to_owned())
        .chars()
        .collect::<Vec<char>>()
    {
        match p {
            'r' => permissions.push(DevicePermissions::Read),
            'w' => permissions.push(DevicePermissions::Write),
            'm' => permissions.push(DevicePermissions::MkNod),
            _ => {}
        }
    }

    Some(DeviceResource {
        allow: d.allow(),
        devtype: dev_type,
        major: d.major().unwrap_or(0),
        minor: d.minor().unwrap_or(0),
        access: permissions,
    })
}

// split flat keyed values into an hashmap of <String, u64>
fn lines_to_map(content: &str) -> HashMap<String, u64> {
    content
        .lines()
        .map(|x| x.split_whitespace().collect::<Vec<&str>>())
        .filter(|x| x.len() == 2 && x[1].parse::<u64>().is_ok())
        .fold(HashMap::new(), |mut hm, x| {
            hm.insert(x[0].to_string(), x[1].parse::<u64>().unwrap());
            hm
        })
}

pub const WILDCARD: i64 = -1;

lazy_static! {
    pub static ref DEFAULT_ALLOWED_DEVICES: Vec<LinuxDeviceCgroup> = {
        vec![
            // all mknod to all char devices
            LinuxDeviceCgroupBuilder::default()
                .allow(true)
                .typ(oci::LinuxDeviceType::C)
                .major(WILDCARD)
                .minor(WILDCARD)
                .access("m")
                .build()
                .unwrap(),

            // all mknod to all block devices
            LinuxDeviceCgroupBuilder::default()
                .allow(true)
                .typ(oci::LinuxDeviceType::B)
                .major(WILDCARD)
                .minor(WILDCARD)
                .access("m")
                .build()
                .unwrap(),

            // all read/write/mknod to char device /dev/console
            LinuxDeviceCgroupBuilder::default()
                .allow(true)
                .typ(oci::LinuxDeviceType::C)
                .major(5)
                .minor(1)
                .access("rwm")
                .build()
                .unwrap(),

            // all read/write/mknod to char device /dev/pts/<N>
            LinuxDeviceCgroupBuilder::default()
                .allow(true)
                .typ(oci::LinuxDeviceType::C)
                .major(136)
                .minor(WILDCARD)
                .access("rwm")
                .build()
                .unwrap(),

            // all read/write/mknod to char device /dev/ptmx
            LinuxDeviceCgroupBuilder::default()
                .allow(true)
                .typ(oci::LinuxDeviceType::C)
                .major(5)
                .minor(2)
                .access("rwm")
                .build()
                .unwrap(),

            // all read/write/mknod to char device /dev/net/tun
            LinuxDeviceCgroupBuilder::default()
                .allow(true)
                .typ(oci::LinuxDeviceType::C)
                .major(10)
                .minor(200)
                .access("rwm")
                .build()
                .unwrap(),
        ]
    };
}

fn get_cpu_stats(cg: &cgroups::Cgroup) -> MessageField<ThrottlingData> {
    let cpu_controller: &CpuController = get_controller_or_return_singular_none!(cg);
    let stat = cpu_controller.cpu().stat;
    let h = lines_to_map(&stat);

    MessageField::some(ThrottlingData {
        periods: *h.get("nr_periods").unwrap_or(&0),
        throttled_periods: *h.get("nr_throttled").unwrap_or(&0),
        throttled_time: *h.get("throttled_time").unwrap_or(&0),
        ..Default::default()
    })
}

fn get_cpu_usage_stats(cg: &cgroups::Cgroup) -> MessageField<CpuUsage> {
    let cpu_controller: &CpuController = get_controller_or_return_singular_none!(cg);
    let stat = cpu_controller.cpu().stat;
    let h = lines_to_map(&stat);
    // cpu.stat uses microseconds; the RPC uses nanoseconds.
    let usage_in_usermode = *h.get("user_usec").unwrap_or(&0) * 1000;
    let usage_in_kernelmode = *h.get("system_usec").unwrap_or(&0) * 1000;
    let total_usage = *h.get("usage_usec").unwrap_or(&0) * 1000;

    MessageField::some(CpuUsage {
        total_usage,
        usage_in_kernelmode,
        usage_in_usermode,
        ..Default::default()
    })
}

// Only send map entries consumed by the host's containerd stats conversion.
fn host_uses_memory_stat(key: &str) -> bool {
    matches!(
        key,
        "inactive_file" | "inactive_anon" | "active_file" | "unevictable"
    )
}

fn get_memory_stats(cg: &cgroups::Cgroup) -> MessageField<MemoryStats> {
    let memory_controller: &MemController = get_controller_or_return_singular_none!(cg);

    // cache from memory stat
    let memory = memory_controller.memory_stat();
    let cache = memory.stat.cache;

    // get memory data
    let usage = MessageField::some(MemoryData {
        usage: memory.usage_in_bytes,
        max_usage: memory.max_usage_in_bytes,
        failcnt: memory.fail_cnt,
        limit: memory.limit_in_bytes as u64,
        ..Default::default()
    });

    // get swap usage
    let memswap = memory_controller.memswap();

    let swap_usage = MessageField::some(MemoryData {
        usage: memswap.usage_in_bytes,
        max_usage: memswap.max_usage_in_bytes,
        failcnt: memswap.fail_cnt,
        limit: memswap.limit_in_bytes as u64,
        ..Default::default()
    });

    MessageField::some(MemoryStats {
        cache,
        usage,
        swap_usage,
        stats: memory
            .stat
            .raw
            .into_iter()
            .filter(|(key, _)| host_uses_memory_stat(key))
            .collect(),
        ..Default::default()
    })
}

fn get_pids_stats(cg: &cgroups::Cgroup) -> MessageField<PidsStats> {
    let pid_controller: &PidController = get_controller_or_return_singular_none!(cg);

    let current = pid_controller.get_pid_current().unwrap_or(0);
    let max = pid_controller.get_pid_max();

    let limit = match max {
        Err(_) => 0,
        Ok(max) => match max {
            MaxValue::Value(v) => v,
            MaxValue::Max => 0,
        },
    } as u64;

    MessageField::some(PidsStats {
        current,
        limit,
        ..Default::default()
    })
}

fn build_blkio_stats_entry(major: i16, minor: i16, op: &str, value: u64) -> BlkioStatsEntry {
    BlkioStatsEntry {
        major: major as u64,
        minor: minor as u64,
        op: op.to_string(),
        value,
        ..Default::default()
    }
}

fn get_blkio_stats(cg: &cgroups::Cgroup) -> MessageField<BlkioStats> {
    let blkio_controller: &BlkIoController = get_controller_or_return_singular_none!(cg);
    let blkio = blkio_controller.blkio();

    let mut resp = BlkioStats::new();
    let mut blkio_stats = Vec::new();

    let stat = blkio.io_stat;
    for s in stat {
        blkio_stats.push(build_blkio_stats_entry(s.major, s.minor, "read", s.rbytes));
        blkio_stats.push(build_blkio_stats_entry(s.major, s.minor, "write", s.wbytes));
        blkio_stats.push(build_blkio_stats_entry(s.major, s.minor, "rios", s.rios));
        blkio_stats.push(build_blkio_stats_entry(s.major, s.minor, "wios", s.wios));
        blkio_stats.push(build_blkio_stats_entry(
            s.major, s.minor, "dbytes", s.dbytes,
        ));
        blkio_stats.push(build_blkio_stats_entry(s.major, s.minor, "dios", s.dios));
    }

    resp.io_service_bytes_recursive = blkio_stats;

    MessageField::some(resp)
}

fn get_hugetlb_stats(cg: &cgroups::Cgroup) -> HashMap<String, HugetlbStats> {
    let mut h = HashMap::new();

    let hugetlb_controller: Option<&HugeTlbController> = cg.controller_of();
    if hugetlb_controller.is_none() {
        return h;
    }
    let hugetlb_controller = hugetlb_controller.unwrap();

    let sizes = hugetlb_controller.get_sizes();
    for size in sizes {
        let usage = hugetlb_controller.usage_in_bytes(&size).unwrap_or(0);
        let max_usage = hugetlb_controller.max_usage_in_bytes(&size).unwrap_or(0);
        let failcnt = hugetlb_controller.failcnt(&size).unwrap_or(0);

        h.insert(
            size.to_string(),
            HugetlbStats {
                usage,
                max_usage,
                failcnt,
                ..Default::default()
            },
        );
    }

    h
}

fn cgroup_path_under_root(root: impl AsRef<Path>, cpath: &str) -> PathBuf {
    root.as_ref().join(cpath.trim_start_matches('/'))
}

fn cgroup_has_init_subcgroup(cpath: &str) -> bool {
    cgroup_path_under_root("/sys/fs/cgroup", cpath)
        .join("init")
        .exists()
}

#[inline]
fn new_cgroup(h: Box<dyn cgroups::Hierarchy>, path: &str) -> Result<Cgroup> {
    let valid_path = path.trim_start_matches('/').to_string();
    cgroups::Cgroup::new(h, valid_path.as_str()).map_err(anyhow::Error::from)
}

#[inline]
fn load_cgroup(h: Box<dyn cgroups::Hierarchy>, path: &str) -> Cgroup {
    let valid_path = path.trim_start_matches('/').to_string();
    cgroups::Cgroup::load(h, valid_path.as_str())
}

impl Manager {
    pub fn new(
        cpath: &str,
        spec: &Spec,
        devcg_info: Option<Arc<RwLock<DevicesCgroupInfo>>>,
    ) -> Result<Self> {
        if !cgroups::hierarchies::is_cgroup2_unified_mode() {
            return Err(anyhow!("kata-fc requires cgroup v2"));
        }
        if let Some(resources) = spec.linux().as_ref().and_then(|l| l.resources().as_ref()) {
            validate_resources(resources)?;
        }

        // Do not expect poisoning lock
        let mut devices_group_info = devcg_info.as_ref().map(|i| i.write().unwrap());
        let pod_cgroup: Option<Cgroup>;

        if let Some(devices_group_info) = devices_group_info.as_mut() {
            // Cgroup path of parent of container
            let pod_cpath = PathBuf::from(cpath)
                .parent()
                .unwrap_or(Path::new("/"))
                .display()
                .to_string();

            if pod_cpath.as_str() == "/" {
                // Skip setting pod cgroup for cpath due to no parent path
                pod_cgroup = None
            } else {
                // Create a cgroup for the pod if not exists.
                // Note that creating pod cgroup MUST be done before the pause
                // container's cgroup created, since the upper node might have
                // some excessive permissions, and children inherit upper
                // node's rules. You'll feel painful to shrink upper nodes'
                // permissions if the new permissions are subset of old.
                pod_cgroup = Some(load_cgroup(
                    Box::new(cgroups::hierarchies::V2::new()),
                    &pod_cpath,
                ));
                let pod_cg = pod_cgroup.as_ref().unwrap();

                let is_allowded_all = Self::has_allowed_all_devices_rule(spec);
                if devices_group_info.inited {
                    debug!(sl(), "Devices cgroup has been initialzied.");

                    // Set allowed all devices to pod cgroup
                    if !devices_group_info.allowed_all && is_allowded_all {
                        info!(
                            sl(),
                            "Pod devices cgroup is changed to allowed all devices mode, devices_group_info = {:?}",
                            devices_group_info
                        );
                        Self::setup_allowed_all_mode(pod_cg).with_context(|| {
                            format!("Setup allowed all devices mode for {pod_cpath}")
                        })?;
                        devices_group_info.allowed_all = true;
                    }
                } else {
                    // This is the first container (aka pause container)
                    debug!(sl(), "Started to init devices cgroup");

                    pod_cg.create().context("Create pod cgroup")?;

                    if !is_allowded_all {
                        Self::setup_devcg_whitelist(pod_cg).with_context(|| {
                            format!("Setup device cgroup whitelist for {pod_cpath}")
                        })?;
                    } else {
                        Self::setup_allowed_all_mode(pod_cg)
                            .with_context(|| format!("Setup allowed all mode for {pod_cpath}"))?;
                        devices_group_info.allowed_all = true;
                    }

                    devices_group_info.inited = true
                }
            }
        } else {
            pod_cgroup = None;
        }

        // Create a cgroup for the container.
        let cg = new_cgroup(Box::new(cgroups::hierarchies::V2::new()), cpath)?;
        // The rules of container cgroup are copied from its parent, which
        // contains some permissions that the container doesn't need.
        // Therefore, resetting the container's devices cgroup is required.
        if let Some(devices_group_info) = devices_group_info.as_ref() {
            if !devices_group_info.allowed_all {
                Self::setup_devcg_whitelist(&cg)
                    .with_context(|| format!("Setup device cgroup whitelist for {cpath}"))?;
            }
        }

        Ok(Self {
            cpath: cpath.to_string(),
            cgroup: cg,
            pod_cgroup,
            devcg_allowed_all: devices_group_info
                .map(|info| info.allowed_all)
                .unwrap_or(false),
        })
    }

    fn cgroup_path_with_subcgroup(&self, subcgroup: &str) -> String {
        let subcgroup = subcgroup.trim_matches('/');
        if subcgroup.is_empty() {
            self.cpath.clone()
        } else {
            Path::new(&self.cpath).join(subcgroup).display().to_string()
        }
    }

    fn setup_allowed_all_mode(cgroup: &cgroups::Cgroup) -> Result<()> {
        // Allow all block and character devices for privileged containers.
        let res = cgroups::Resources {
            devices: cgroups::DeviceResources {
                devices: vec![
                    DeviceResource {
                        allow: true,
                        devtype: DeviceType::Block,
                        major: -1,
                        minor: -1,
                        access: vec![
                            DevicePermissions::Read,
                            DevicePermissions::Write,
                            DevicePermissions::MkNod,
                        ],
                    },
                    DeviceResource {
                        allow: true,
                        devtype: DeviceType::Char,
                        major: -1,
                        minor: -1,
                        access: vec![
                            DevicePermissions::Read,
                            DevicePermissions::Write,
                            DevicePermissions::MkNod,
                        ],
                    },
                ],
            },
            ..Default::default()
        };
        cgroup.apply(&res)?;

        Ok(())
    }

    /// Setup device cgroup whitelist:
    /// - Deny all devices in order to cleanup device cgroup.
    /// - Allow default devices and default allowed devices.
    fn setup_devcg_whitelist(cgroup: &cgroups::Cgroup) -> Result<()> {
        #[allow(unused_mut)]
        let mut dev_res_list = vec![DeviceResource {
            allow: false,
            devtype: DeviceType::All,
            major: -1,
            minor: -1,
            access: vec![
                DevicePermissions::Read,
                DevicePermissions::Write,
                DevicePermissions::MkNod,
            ],
        }];
        // Do not append default allowed devices for simplicity while
        // testing.
        #[cfg(not(test))]
        dev_res_list.append(&mut default_allowed_devices());

        let res = cgroups::Resources {
            devices: cgroups::DeviceResources {
                devices: dev_res_list,
            },
            ..Default::default()
        };
        cgroup.apply(&res)?;

        Ok(())
    }

    /// Check if OCI spec contains a rule of allowed all devices.
    fn has_allowed_all_devices_rule(spec: &Spec) -> bool {
        let linux = match spec.linux().as_ref() {
            Some(linux) => linux,
            None => return false,
        };
        let resources = match linux.resources().as_ref() {
            Some(resource) => resource,
            None => return false,
        };

        resources
            .devices()
            .as_ref()
            .and_then(|devices| {
                devices
                    .iter()
                    .find(|dev| rule_for_all_devices(dev))
                    .map(|dev| dev.allow())
            })
            .unwrap_or_default()
    }
}

/// Generate a list for allowed devices including `DEFAULT_DEVICES` and
/// `DEFAULT_ALLOWED_DEVICES`.
fn default_allowed_devices() -> Vec<DeviceResource> {
    let mut dev_res_list = Vec::new();
    DEFAULT_DEVICES.iter().for_each(|dev| {
        if let Some(dev_res) = linux_device_to_device_resource(dev) {
            dev_res_list.push(dev_res)
        }
    });
    DEFAULT_ALLOWED_DEVICES.iter().for_each(|dev| {
        if let Some(dev_res) = linux_device_cgroup_to_device_resource(dev) {
            dev_res_list.push(dev_res)
        }
    });
    dev_res_list
}

/// Convert LinuxDevice to DeviceResource.
fn linux_device_to_device_resource(d: &LinuxDevice) -> Option<DeviceResource> {
    let dev_type = DeviceType::from_char(d.typ().as_str().chars().next())?;

    let permissions = vec![
        DevicePermissions::Read,
        DevicePermissions::Write,
        DevicePermissions::MkNod,
    ];

    Some(DeviceResource {
        allow: true,
        devtype: dev_type,
        major: d.major(),
        minor: d.minor(),
        access: permissions,
    })
}

// Since the OCI spec is designed for cgroup v1, in some cases
// there is need to convert from the cgroup v1 configuration to cgroup v2
// the formula for cpuShares is y = (1 + ((x - 2) * 9999) / 262142)
// convert from [2-262144] to [1-10000]
// 262144 comes from Linux kernel definition "#define MAX_SHARES (1UL << 18)"
// from https://github.com/opencontainers/runc/blob/a5847db387ae28c0ca4ebe4beee1a76900c86414/libcontainer/cgroups/utils.go#L394
pub fn convert_shares_to_v2_value(shares: u64) -> u64 {
    if shares == 0 {
        return 0;
    }
    1 + ((shares - 2) * 9999) / 262142
}

// ConvertMemorySwapToCgroupV2Value converts MemorySwap value from OCI spec
// for use by cgroup v2 drivers. A conversion is needed since Resources.MemorySwap
// is defined as memory+swap combined, while in cgroup v2 swap is a separate value.
fn convert_memory_swap_to_v2_value(memory_swap: i64, memory: i64) -> Result<i64> {
    // for compatibility with cgroup1 controller, set swap to unlimited in
    // case the memory is set to unlimited, and swap is not explicitly set,
    // treating the request as "set both memory and swap to unlimited".
    if memory == -1 && memory_swap == 0 {
        return Ok(-1);
    }
    if memory_swap == -1 || memory_swap == 0 {
        // -1 is "max", 0 is "unset", so treat as is
        return Ok(memory_swap);
    }
    // sanity checks
    if memory == 0 || memory == -1 {
        return Err(anyhow!("unable to set swap limit without memory limit"));
    }
    if memory < 0 {
        return Err(anyhow!("invalid memory value: {}", memory));
    }
    if memory_swap < memory {
        return Err(anyhow!("memory+swap limit should be >= memory limit"));
    }
    Ok(memory_swap - memory)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::PathBuf;

    use crate::cgroups_rs as cgroups;
    use cgroups::devices::{DevicePermissions, DeviceType};

    use super::{cgroup_path_under_root, default_allowed_devices, load_cgroup};
    use crate::cgroups::fs::{lines_to_map, Manager, DEFAULT_ALLOWED_DEVICES, WILDCARD};
    use crate::container::DEFAULT_DEVICES;

    #[test]
    fn test_cgroup_path_under_root_trims_absolute_cpath() {
        assert_eq!(
            cgroup_path_under_root(
                "/sys/fs/cgroup",
                "/docker.slice/docker-containers.slice/container"
            ),
            PathBuf::from("/sys/fs/cgroup/docker.slice/docker-containers.slice/container")
        );
    }

    #[test]
    fn test_cgroup_path_with_subcgroup_preserves_absolute_cpath() {
        let manager = Manager {
            cpath: "/docker.slice/docker-containers.slice/container".to_string(),
            cgroup: load_cgroup(Box::new(cgroups::hierarchies::V2::new()), "/"),
            pod_cgroup: None,
            devcg_allowed_all: false,
        };

        assert_eq!(
            manager.cgroup_path_with_subcgroup("/init/"),
            "/docker.slice/docker-containers.slice/container/init"
        );
        assert_eq!(
            manager.cgroup_path_with_subcgroup("/"),
            "/docker.slice/docker-containers.slice/container"
        );
    }

    #[test]
    fn test_lines_to_map() {
        let hm1: HashMap<String, u64> = [
            ("a".to_string(), 1),
            ("b".to_string(), 2),
            ("c".to_string(), 3),
            ("e".to_string(), 5),
        ]
        .iter()
        .cloned()
        .collect();
        let hm2: HashMap<String, u64> = [("a".to_string(), 1)].iter().cloned().collect();

        let test_cases = vec![
            ("a 1\nb 2\nc 3\nd X\ne 5\n", hm1),
            ("a 1", hm2),
            ("a c", HashMap::new()),
        ];

        for test_case in test_cases {
            let result = lines_to_map(test_case.0);
            assert_eq!(
                result, test_case.1,
                "except: {:?} for input {}",
                test_case.1, test_case.0
            );
        }
    }

    #[test]
    fn test_reject_cgroup_v1_resources() {
        for request in [
            serde_json::json!({"network": {"classID": 1}}),
            serde_json::json!({"network": {"priorities": [{"name": "eth0", "priority": 1}]}}),
            serde_json::json!({"cpu": {"realtimePeriod": 1}}),
            serde_json::json!({"cpu": {"realtimeRuntime": 1}}),
            serde_json::json!({"memory": {"kernel": 1}}),
            serde_json::json!({"memory": {"kernelTCP": 1}}),
            serde_json::json!({"memory": {"swappiness": 0}}),
            serde_json::json!({"memory": {"disableOOMKiller": true}}),
            serde_json::json!({"blockIO": {"leafWeight": 1}}),
            serde_json::json!({"blockIO": {"weightDevice": [{"major": 8, "minor": 0, "leafWeight": 1}]}}),
        ] {
            let resources = serde_json::from_value(request.clone()).unwrap();
            assert!(
                super::validate_resources(&resources).is_err(),
                "accepted {request}"
            );
        }
        let resources = serde_json::from_value(serde_json::json!({
            "cpu": {"period": 100000, "quota": 100000, "shares": 1024},
            "memory": {"limit": 536870912, "swap": 536870912, "disableOOMKiller": false},
            "pids": {"limit": 256},
            "hugepageLimits": [{"pageSize": "2MB", "limit": 2097152}]
        }))
        .unwrap();
        super::validate_resources(&resources).unwrap();
    }

    #[test]
    fn test_memory_swap_v2_limit() {
        for (combined, memory, expected) in [
            (0, 0, 0),
            (0, -1, -1),
            (-1, 512, -1),
            (512, 512, 0),
            (1024, 512, 512),
        ] {
            assert_eq!(
                super::convert_memory_swap_to_v2_value(combined, memory).unwrap(),
                expected
            );
        }
        for (combined, memory) in [(100, 0), (100, -1), (100, 200), (100, -2)] {
            assert!(super::convert_memory_swap_to_v2_value(combined, memory).is_err());
        }
    }

    #[test]
    fn test_default_allowed_devices() {
        let allowed_devices = default_allowed_devices();
        assert_eq!(
            allowed_devices.len(),
            DEFAULT_DEVICES.len() + DEFAULT_ALLOWED_DEVICES.len()
        );

        let allowed_permissions = [
            DevicePermissions::Read,
            DevicePermissions::Write,
            DevicePermissions::MkNod,
        ];

        let default_devices_0 = &allowed_devices[0];
        assert!(default_devices_0.allow);
        assert_eq!(default_devices_0.devtype, DeviceType::Char);
        assert_eq!(default_devices_0.major, 1);
        assert_eq!(default_devices_0.minor, 3);
        assert!(default_devices_0
            .access
            .iter()
            .all(|&p| allowed_permissions.contains(&p)));

        let default_allowed_devices_0 = &allowed_devices[DEFAULT_DEVICES.len()];
        assert!(default_allowed_devices_0.allow);
        assert_eq!(default_allowed_devices_0.devtype, DeviceType::Char);
        assert_eq!(default_allowed_devices_0.major, WILDCARD);
        assert_eq!(default_allowed_devices_0.minor, WILDCARD);
        assert_eq!(
            default_allowed_devices_0.access,
            vec![DevicePermissions::MkNod]
        );
    }
}
