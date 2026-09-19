// Copyright (c) 2019 Ant Financial
//
// SPDX-License-Identifier: Apache-2.0
//

pub const PROC_MOUNTSTATS: &str = "/proc/self/mountstats";

// Linux UEvent related consts.
pub const U_EVENT_ACTION: &str = "ACTION";
pub const U_EVENT_ACTION_ADD: &str = "add";
pub const U_EVENT_ACTION_REMOVE: &str = "remove";
pub const U_EVENT_DEV_PATH: &str = "DEVPATH";
pub const U_EVENT_SUB_SYSTEM: &str = "SUBSYSTEM";
pub const U_EVENT_SEQ_NUM: &str = "SEQNUM";
pub const U_EVENT_DEV_NAME: &str = "DEVNAME";
pub const U_EVENT_INTERFACE: &str = "INTERFACE";
