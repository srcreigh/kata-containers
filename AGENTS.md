# Firecracker reduction maintenance

The README defines the supported contract. Reject excluded features explicitly;
do not silently fall back to another VMM, backend or host container.
Preserve jailer/seccomp, safe paths, namespace/cgroup isolation, credentials and
upstream security fixes. Source-size reduction is not a security proof.

Build and test on the supported native Linux target. Keep privileged kernel tests
isolated. Record source identity, relevant regression evidence and limitations.
Keep personal metadata, deployment inventories and operator-specific scripts out
of this source repository. Explain behavioral corrections with their regression
cause and evidence in docs/additive-fixes.
