// Copyright (c) 2019 Ant Financial
//
// SPDX-License-Identifier: Apache-2.0
//

// From https://www.kernel.org/doc/Documentation/acpi/namespace.txt
// The Linux kernel's core ACPI subsystem creates struct acpi_device
// objects for ACPI namespace objects representing devices, power resources
// processors, thermal zones. Those objects are exported to user space via
// sysfs as directories in the subtree under /sys/devices/LNXSYSTM:00
pub const ACPI_DEV_PATH: &str = "/devices/LNXSYSTM";

pub const SYSFS_CPU_PATH: &str = "/sys/devices/system/cpu";
pub const SYSFS_CPU_ONLINE_PATH: &str = "/sys/devices/system/cpu/online";

pub const SYSFS_MEMORY_BLOCK_SIZE_PATH: &str = "/sys/devices/system/memory/block_size_bytes";
pub const SYSFS_MEMORY_HOTPLUG_PROBE_PATH: &str = "/sys/devices/system/memory/probe";
pub const SYSFS_MEMORY_ONLINE_PATH: &str = "/sys/devices/system/memory";

pub const SYSFS_SCSI_HOST_PATH: &str = "/sys/class/scsi_host";
pub const SYSFS_NET_PATH: &str = "/sys/class/net";

pub const SYSFS_BUS_PCI_PATH: &str = "/sys/bus/pci";

pub const SYSFS_CGROUPPATH: &str = "/sys/fs/cgroup";
pub const SYSFS_ONLINE_FILE: &str = "online";

pub const PROC_MOUNTSTATS: &str = "/proc/self/mountstats";
pub const PROC_CGROUPS: &str = "/proc/cgroups";

pub const SYSTEM_DEV_PATH: &str = "/dev";

// Linux UEvent related consts.
pub const U_EVENT_ACTION: &str = "ACTION";
pub const U_EVENT_ACTION_ADD: &str = "add";
pub const U_EVENT_ACTION_REMOVE: &str = "remove";
pub const U_EVENT_DEV_PATH: &str = "DEVPATH";
pub const U_EVENT_SUB_SYSTEM: &str = "SUBSYSTEM";
pub const U_EVENT_SEQ_NUM: &str = "SEQNUM";
pub const U_EVENT_DEV_NAME: &str = "DEVNAME";
pub const U_EVENT_INTERFACE: &str = "INTERFACE";
