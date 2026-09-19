// Copyright (c) 2019-2021 Alibaba Cloud
// Copyright (c) 2019-2021 Ant Group
//
// SPDX-License-Identifier: Apache-2.0
//

use std::path::Path;

use crate::annotations;
use crate::container::ContainerType;
use oci_spec::runtime as oci;
use std::str::FromStr;
// K8S_EMPTY_DIR is the K8s specific path for `empty-dir` volumes
const K8S_EMPTY_DIR: &str = "kubernetes.io~empty-dir";
// K8S_CONFIGMAP is the K8s specific path for `configmap` volumes
const K8S_CONFIGMAP: &str = "kubernetes.io~configmap";
// K8S_SECRET is the K8s specific path for `secret` volumes
const K8S_SECRET: &str = "kubernetes.io~secret";
// K8S_PROJECTED is the K8s specific path for `projected` volumes
const K8S_PROJECTED: &str = "kubernetes.io~projected";
// K8S_DOWNWARD_API is the K8s specific path for `downward-api` volumes
const K8S_DOWNWARD_API: &str = "kubernetes.io~downward-api";

/// Check whether the path is a K8s empty directory.
pub fn is_empty_dir<P: AsRef<Path>>(path: P) -> bool {
    is_special_dir(path, K8S_EMPTY_DIR)
}

/// Check whether the path is a K8s configmap.
pub fn is_configmap<P: AsRef<Path>>(path: P) -> bool {
    is_special_dir(path, K8S_CONFIGMAP)
}

/// Check whether the path is a K8s secret.
pub fn is_secret<P: AsRef<Path>>(path: P) -> bool {
    is_special_dir(path, K8S_SECRET)
}

/// Check whether the path is a K8s projected volume.
pub fn is_projected<P: AsRef<Path>>(path: P) -> bool {
    is_special_dir(path, K8S_PROJECTED)
}

/// Check whether the path is a K8s downward-api volume.
pub fn is_downward_api<P: AsRef<Path>>(path: P) -> bool {
    is_special_dir(path, K8S_DOWNWARD_API)
}

/// Check whether the path is a K8s empty directory, configmap, or secret.
///
/// For example, given a K8s EmptyDir, Kubernetes mounts
/// "/var/lib/kubelet/pods/<id>/volumes/kubernetes.io~empty-dir/<volumeMount name>"
/// to "/<mount-point>".
pub fn is_special_dir<P: AsRef<Path>>(path: P, dir_type: &str) -> bool {
    let path = path.as_ref();

    if let Some(parent) = path.parent() {
        if let Some(pname) = parent.file_name() {
            if pname == dir_type && parent.parent().is_some() {
                return true;
            }
        }
    }

    false
}

/// Get K8S container type from OCI annotations.
pub fn container_type(spec: &oci::Spec) -> ContainerType {
    // PodSandbox:  "sandbox" (Containerd & CRI-O), "podsandbox" (dockershim)
    // PodContainer: "container" (Containerd & CRI-O & dockershim)
    for k in [
        annotations::crio::CONTAINER_TYPE_LABEL_KEY,
        annotations::cri_containerd::CONTAINER_TYPE_LABEL_KEY,
        annotations::dockershim::CONTAINER_TYPE_LABEL_KEY,
    ]
    .iter()
    {
        if let Some(annotations) = spec.annotations() {
            if let Some(v) = annotations.get(k.to_owned()) {
                if let Ok(t) = ContainerType::from_str(v) {
                    return t;
                }
            }
        }
    }

    ContainerType::SingleContainer
}

/// Determine the k8s sandbox ID from OCI annotations.
///
/// This function is expected to be called only when the container type is "PodContainer".
pub fn container_type_with_id(spec: &oci::Spec) -> (ContainerType, Option<String>) {
    let container_type = container_type(spec);
    let mut sid = None;
    if container_type == ContainerType::PodContainer {
        for k in [
            annotations::crio::SANDBOX_ID_LABEL_KEY,
            annotations::cri_containerd::SANDBOX_ID_LABEL_KEY,
            annotations::dockershim::SANDBOX_ID_LABEL_KEY,
        ]
        .iter()
        {
            if let Some(annotations) = spec.annotations() {
                if let Some(id) = annotations.get(k.to_owned()) {
                    sid = Some(id.to_string());
                    break;
                }
            }
        }
    }

    (container_type, sid)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{annotations, container};

    #[test]
    fn test_is_empty_dir() {
        let empty_dir = "/volumes/kubernetes.io~empty-dir/shm";
        assert!(is_empty_dir(empty_dir));

        let empty_dir = "/volumes/kubernetes.io~empty-dir//shm";
        assert!(is_empty_dir(empty_dir));

        let empty_dir = "/volumes/kubernetes.io~empty-dir-test/shm";
        assert!(!is_empty_dir(empty_dir));

        let empty_dir = "/volumes/kubernetes.io~empty-dir";
        assert!(!is_empty_dir(empty_dir));

        let empty_dir = "kubernetes.io~empty-dir";
        assert!(!is_empty_dir(empty_dir));

        let empty_dir = "/kubernetes.io~empty-dir/shm";
        assert!(is_empty_dir(empty_dir));
    }

    #[test]
    fn test_is_configmap() {
        let path = "/volumes/kubernetes.io~configmap/cm";
        assert!(is_configmap(path));

        let path = "/volumes/kubernetes.io~configmap//cm";
        assert!(is_configmap(path));

        let path = "/volumes/kubernetes.io~configmap-test/cm";
        assert!(!is_configmap(path));

        let path = "/volumes/kubernetes.io~configmap";
        assert!(!is_configmap(path));
    }

    #[test]
    fn test_is_secret() {
        let path = "/volumes/kubernetes.io~secret/test-serect";
        assert!(is_secret(path));

        let path = "/volumes/kubernetes.io~secret//test-serect";
        assert!(is_secret(path));

        let path = "/volumes/kubernetes.io~secret-test/test-serect";
        assert!(!is_secret(path));

        let path = "/volumes/kubernetes.io~secret";
        assert!(!is_secret(path));
    }

    #[test]
    fn test_is_projected() {
        let path = "/volumes/kubernetes.io~projected/foo";
        assert!(is_projected(path));

        let path = "/volumes/kubernetes.io~projected//foo";
        assert!(is_projected(path));

        let path = "/volumes/kubernetes.io~projected-test/foo";
        assert!(!is_projected(path));

        let path = "/volumes/kubernetes.io~projected";
        assert!(!is_projected(path));
    }

    #[test]
    fn test_is_downward_api() {
        let path = "/volumes/kubernetes.io~downward-api/foo";
        assert!(is_downward_api(path));

        let path = "/volumes/kubernetes.io~downward-api//foo";
        assert!(is_downward_api(path));

        let path = "/volumes/kubernetes.io~downward-api-test/foo";
        assert!(!is_downward_api(path));

        let path = "/volumes/kubernetes.io~downward-api";
        assert!(!is_downward_api(path));
    }

    #[test]
    fn test_container_type() {
        let sid = "sid".to_string();
        let mut spec = oci::Spec::default();

        // default
        assert_eq!(
            container_type_with_id(&spec),
            (ContainerType::SingleContainer, None)
        );

        // crio sandbox
        spec.set_annotations(Some(
            [(
                annotations::crio::CONTAINER_TYPE_LABEL_KEY.to_string(),
                container::SANDBOX.to_string(),
            )]
            .iter()
            .cloned()
            .collect(),
        ));
        assert_eq!(
            container_type_with_id(&spec),
            (ContainerType::PodSandbox, None)
        );

        // cri containerd sandbox
        spec.set_annotations(Some(
            [(
                annotations::crio::CONTAINER_TYPE_LABEL_KEY.to_string(),
                container::POD_SANDBOX.to_string(),
            )]
            .iter()
            .cloned()
            .collect(),
        ));
        assert_eq!(
            container_type_with_id(&spec),
            (ContainerType::PodSandbox, None)
        );

        // docker shim sandbox
        spec.set_annotations(Some(
            [(
                annotations::crio::CONTAINER_TYPE_LABEL_KEY.to_string(),
                container::PODSANDBOX.to_string(),
            )]
            .iter()
            .cloned()
            .collect(),
        ));
        assert_eq!(
            container_type_with_id(&spec),
            (ContainerType::PodSandbox, None)
        );

        // crio container
        spec.set_annotations(Some(
            [
                (
                    annotations::crio::CONTAINER_TYPE_LABEL_KEY.to_string(),
                    container::CONTAINER.to_string(),
                ),
                (
                    annotations::crio::SANDBOX_ID_LABEL_KEY.to_string(),
                    sid.clone(),
                ),
            ]
            .iter()
            .cloned()
            .collect(),
        ));
        assert_eq!(
            container_type_with_id(&spec),
            (ContainerType::PodContainer, Some(sid.clone()))
        );

        // cri containerd container
        spec.set_annotations(Some(
            [
                (
                    annotations::cri_containerd::CONTAINER_TYPE_LABEL_KEY.to_string(),
                    container::POD_CONTAINER.to_string(),
                ),
                (
                    annotations::cri_containerd::SANDBOX_ID_LABEL_KEY.to_string(),
                    sid.clone(),
                ),
            ]
            .iter()
            .cloned()
            .collect(),
        ));
        assert_eq!(
            container_type_with_id(&spec),
            (ContainerType::PodContainer, Some(sid.clone()))
        );

        // docker shim container
        spec.set_annotations(Some(
            [
                (
                    annotations::dockershim::CONTAINER_TYPE_LABEL_KEY.to_string(),
                    container::CONTAINER.to_string(),
                ),
                (
                    annotations::dockershim::SANDBOX_ID_LABEL_KEY.to_string(),
                    sid.clone(),
                ),
            ]
            .iter()
            .cloned()
            .collect(),
        ));
        assert_eq!(
            container_type_with_id(&spec),
            (ContainerType::PodContainer, Some(sid))
        );
    }
}
