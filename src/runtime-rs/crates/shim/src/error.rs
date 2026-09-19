// Copyright (c) 2019-2022 Alibaba Cloud
// Copyright (c) 2019-2022 Ant Group
//
// SPDX-License-Identifier: Apache-2.0
//

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("failed to parse argument {0}")]
    ParseArgument(String),
    #[error("invalid argument")]
    InvalidArgument,
    #[error("argument is empty {0}")]
    ArgumentIsEmpty(String),

    // File
    #[error("failed to write file {0}")]
    FileWrite(String),

    #[error("empty sandbox id")]
    EmptySandboxId,
    #[error("failed to get self exec: {0}")]
    SelfExec(#[source] std::io::Error),
    #[error("failed to spawn child: {0}")]
    SpawnChild(#[source] std::io::Error),
    #[error("failed to get env variable: {0}")]
    EnvVar(#[source] std::env::VarError),
    #[error("failed to parse server fd environment variable {0}")]
    ServerFd(String),
    #[error("failed to get system time: {0}")]
    SystemTime(#[source] std::time::SystemTimeError),
}
