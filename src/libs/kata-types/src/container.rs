// Copyright (c) 2019-2021 Alibaba Cloud
// Copyright (c) 2019-2021 Ant Group
//
// SPDX-License-Identifier: Apache-2.0
//

use std::collections::HashMap;
use std::fmt::{Display, Formatter};
use std::str::FromStr;

/// a container running within a pod
pub const POD_CONTAINER: &str = "pod_container";
// cri containerd/crio/docker: a container running within a pod
pub(crate) const CONTAINER: &str = "container";

/// a pod sandbox container
pub const POD_SANDBOX: &str = "pod_sandbox";
// cri containerd/crio: a pod sandbox container
pub(crate) const SANDBOX: &str = "sandbox";
// docker: a sandbox sandbox container
pub(crate) const PODSANDBOX: &str = "podsandbox";

pub(crate) const SINGLE_CONTAINER: &str = "single_container";

/// Error codes for container related operations.
#[derive(thiserror::Error, Debug)]
pub enum Error {
    /// Invalid container type
    #[error("Invalid container type {0}")]
    InvalidContainerType(String),
}

/// Types of pod containers: container or sandbox.
#[derive(PartialEq, Debug, Clone)]
pub enum ContainerType {
    /// A pod container.
    PodContainer,
    /// A pod sandbox.
    PodSandbox,
    /// A single container.
    SingleContainer,
}

impl Display for ContainerType {
    fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
        match self {
            ContainerType::PodContainer => write!(f, "{POD_CONTAINER}"),
            ContainerType::PodSandbox => write!(f, "{POD_SANDBOX}"),
            ContainerType::SingleContainer => write!(f, "{SINGLE_CONTAINER}"),
        }
    }
}

impl FromStr for ContainerType {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            POD_CONTAINER | CONTAINER => Ok(ContainerType::PodContainer),
            POD_SANDBOX | PODSANDBOX | SANDBOX => Ok(ContainerType::PodSandbox),
            SINGLE_CONTAINER => Ok(ContainerType::SingleContainer),
            _ => Err(Error::InvalidContainerType(value.to_owned())),
        }
    }
}

/// Updates OCI annotations by removing specified keys and adding new ones.
///
/// This function creates a new `HashMap` containing the updated annotations, ensuring
/// that the original map remains unchanged. It is optimized for performance by
/// pre-allocating the necessary capacity and handling removals and additions
/// efficiently.
///
/// **Note:** If a key from `keys_to_add` already exists in the new map (either from
/// the original annotations or from another entry in `keys_to_add`), the
/// corresponding add operation will be **ignored**. The existing value will not be
/// overwritten.
///
/// # Arguments
///
/// * `annotations` - A reference to the original `HashMap` of annotations.
/// * `keys_to_remove` - A slice of string slices (`&[&str]`) representing the keys to be removed.
/// * `keys_to_add` - A slice of tuples (`&[(String, String)]`) representing the new key-value pairs to add.
///
/// # Returns
///
/// A new `HashMap` containing the merged and updated set of annotations.
///
pub fn update_ocispec_annotations(
    annotations: &HashMap<String, String>,
    keys_to_remove: &[&str],
    keys_to_add: &[(String, String)],
) -> HashMap<String, String> {
    // 1. Validate input parameters
    if keys_to_remove.is_empty() && keys_to_add.is_empty() {
        return annotations.clone();
    }

    // Pre-allocate capacity: Original size + number of additions
    let mut updated = HashMap::with_capacity(annotations.len() + keys_to_add.len());

    // 2. Copy retained key-value pairs (use `contains` directly)
    for (k, v) in annotations {
        if !keys_to_remove.iter().any(|&rm| rm == k) {
            updated.insert(k.clone(), v.clone());
        }
    }

    // 3. Add new key-value pairs
    for (key, value) in keys_to_add {
        if !updated.contains_key(key) {
            updated.insert(key.clone(), value.clone());
        }
    }

    updated
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_container_type_display() {
        assert_eq!(format!("{}", ContainerType::PodContainer), POD_CONTAINER);
        assert_eq!(format!("{}", ContainerType::PodSandbox), POD_SANDBOX);
    }

    #[test]
    fn test_container_type_from_str() {
        assert_eq!(
            ContainerType::from_str("pod_container").unwrap(),
            ContainerType::PodContainer
        );
        assert_eq!(
            ContainerType::from_str("container").unwrap(),
            ContainerType::PodContainer
        );
        assert_eq!(
            ContainerType::from_str("pod_sandbox").unwrap(),
            ContainerType::PodSandbox
        );
        assert_eq!(
            ContainerType::from_str("podsandbox").unwrap(),
            ContainerType::PodSandbox
        );
        assert_eq!(
            ContainerType::from_str("sandbox").unwrap(),
            ContainerType::PodSandbox
        );
        assert_eq!(
            ContainerType::from_str("single_container").unwrap(),
            ContainerType::SingleContainer
        );
        ContainerType::from_str("test").unwrap_err();
    }

    #[test]
    fn test_update_annotations_combined_operations() {
        // Initial annotations
        let mut annotations = HashMap::new();
        annotations.insert("key1".to_string(), "val1".to_string());
        annotations.insert("key2".to_string(), "val2".to_string());

        // Keys to remove and add
        let keys_to_remove = ["key2"];
        let keys_to_add = vec![
            ("key3".to_string(), "val3".to_string()),
            ("key1".to_string(), "new_val1".to_string()), // Attempt to overwrite
        ];

        // Call the function with keys_to_remove and keys_to_add
        let updated = update_ocispec_annotations(&annotations, &keys_to_remove, &keys_to_add);

        // Assert the result
        assert_eq!(updated.len(), 2);
        assert_eq!(updated.get("key1"), Some(&"val1".to_string())); // Not overwritten
        assert!(!updated.contains_key("key2")); // Removed
        assert_eq!(updated.get("key3"), Some(&"val3".to_string())); // Added
    }

    #[test]
    fn test_update_annotations_whith_empty_inputs() {
        // Initial annotations
        let annotations = HashMap::new();

        // empty inputs
        let updated = update_ocispec_annotations(&annotations, &[], &[]);

        // Assert the result
        assert_eq!(annotations, updated);
    }

    #[test]
    fn test_update_annotations_not_in_annotations_map() {
        // Initial annotations
        let mut annotations = HashMap::new();
        annotations.insert("key1".to_string(), "val1".to_string());
        annotations.insert("key2".to_string(), "val2".to_string());

        // Keys to remove and add
        let keys_to_remove = ["key4"];
        let keys_to_add = vec![
            ("key3".to_string(), "val3".to_string()),
            ("key1".to_string(), "new_val1".to_string()), // Attempt to overwrite
        ];

        // a `keys_to_remove` that isn't currently in the annotations map is passed in with "key4"
        let updated = update_ocispec_annotations(&annotations, &keys_to_remove, &keys_to_add);

        // Assert the result
        assert_eq!(updated.len(), 3);
        assert_eq!(updated.get("key1"), Some(&"val1".to_string())); // Not overwritten
        assert_eq!(updated.get("key2"), Some(&"val2".to_string())); // keep it
        assert_eq!(updated.get("key3"), Some(&"val3".to_string())); // Added
    }
}
