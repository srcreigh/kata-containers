// Copyright (c) 2019-2021 Alibaba Cloud
// SPDX-License-Identifier: Apache-2.0

use kata_types::annotations::{
    Annotation, KATA_ANNO_CFG_AGENT_TRACE, KATA_ANNO_CFG_HYPERVISOR_DEFAULT_MEMORY,
    KATA_ANNO_CFG_HYPERVISOR_DEFAULT_VCPUS,
};
use kata_types::config::{FirecrackerConfig, TomlConfig};

fn config() -> TomlConfig {
    FirecrackerConfig::new().register();
    TomlConfig::load(
        r#"
        [hypervisor.firecracker]
        path = "/dev/null"
        kernel = "/dev/null"
        image = "/dev/null"
        default_vcpus = 1
        default_memory = 128
        enable_annotations = ["default_vcpus", "default_memory"]
        [agent.kata]
        [runtime]
        name = "virt-container"
        hypervisor_name = "firecracker"
        agent_name = "kata"
        sandbox_cgroup_only = true
        "#,
    )
    .unwrap()
}

#[test]
fn sizing_and_tracing_annotations() {
    let mut cfg = config();
    Annotation::new(
        [
            (KATA_ANNO_CFG_HYPERVISOR_DEFAULT_VCPUS.into(), "2".into()),
            (
                KATA_ANNO_CFG_HYPERVISOR_DEFAULT_MEMORY.into(),
                "256MiB".into(),
            ),
            (KATA_ANNO_CFG_AGENT_TRACE.into(), "true".into()),
            (
                "io.katacontainers.config.runtime.enable_tracing".into(),
                "true".into(),
            ),
        ]
        .into(),
    )
    .update_config_by_annotation(&mut cfg)
    .unwrap();
    assert_eq!(cfg.hypervisor["firecracker"].cpu_info.default_vcpus, 2.0);
    assert_eq!(
        cfg.hypervisor["firecracker"].memory_info.default_memory,
        256
    );
    assert!(cfg.agent["kata"].enable_tracing);
    assert!(cfg.runtime.enable_tracing);
}

#[test]
fn disabled_sizing_annotations_are_ignored() {
    let mut cfg = config();
    cfg.hypervisor
        .get_mut("firecracker")
        .unwrap()
        .security_info
        .enable_annotations
        .clear();
    Annotation::new(
        [
            (
                KATA_ANNO_CFG_HYPERVISOR_DEFAULT_VCPUS.into(),
                "invalid".into(),
            ),
            (KATA_ANNO_CFG_HYPERVISOR_DEFAULT_MEMORY.into(), "10".into()),
        ]
        .into(),
    )
    .update_config_by_annotation(&mut cfg)
    .unwrap();
    assert_eq!(cfg.hypervisor["firecracker"].cpu_info.default_vcpus, 1.0);
    assert_eq!(
        cfg.hypervisor["firecracker"].memory_info.default_memory,
        128
    );
}

#[test]
fn invalid_sizing_and_tracing_annotations_fail() {
    for (key, value) in [
        (KATA_ANNO_CFG_HYPERVISOR_DEFAULT_MEMORY, "10"),
        (KATA_ANNO_CFG_HYPERVISOR_DEFAULT_VCPUS, "400"),
        (KATA_ANNO_CFG_HYPERVISOR_DEFAULT_VCPUS, "invalid"),
        (KATA_ANNO_CFG_AGENT_TRACE, "invalid"),
        ("io.katacontainers.config.runtime.enable_tracing", "invalid"),
    ] {
        assert!(
            Annotation::new([(key.into(), value.into())].into())
                .update_config_by_annotation(&mut config())
                .is_err(),
            "{}",
            key
        );
    }
}

#[test]
fn unsupported_overrides_fail_even_when_enabled() {
    for key in [
        "io.katacontainers.config.hypervisor.path",
        "io.katacontainers.config.hypervisor.jailer_path",
        "io.katacontainers.config.hypervisor.kernel_params",
        "io.katacontainers.config.hypervisor.virtio_fs_daemon",
        "io.katacontainers.config.hypervisor.cc_init_data",
        "io.katacontainers.config.hypervisor.blk_logical_sector_size",
        "io.katacontainers.config.agent.kernel_modules",
        "io.katacontainers.config.runtime.disable_guest_seccomp",
        "io.katacontainers.config.runtime.name",
        "io.katacontainers.config.runtime.shared_mounts",
    ] {
        let mut cfg = config();
        cfg.hypervisor
            .get_mut("firecracker")
            .unwrap()
            .security_info
            .enable_annotations = vec![".*".into()];
        assert!(
            Annotation::new([(key.into(), "true".into())].into())
                .update_config_by_annotation(&mut cfg)
                .is_err(),
            "{}",
            key
        );
    }
}

#[test]
fn unrelated_kubernetes_annotations_are_ignored() {
    Annotation::new([("io.kubernetes.cri.container-type".into(), "sandbox".into())].into())
        .update_config_by_annotation(&mut config())
        .unwrap();
}
