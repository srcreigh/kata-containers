// Copyright (c) 2021 Alibaba Cloud
//
// SPDX-License-Identifier: Apache-2.0
//

#[macro_use]
extern crate slog;

pub mod fs;
pub mod guest_io;
pub mod k8s;
pub mod mount;
pub mod netns;
pub mod rand;
pub mod spec;
pub mod validate;

// Convenience macro to obtain the scoped logger
#[macro_export]
macro_rules! sl {
    () => {
        slog_scope::logger()
    };
}
