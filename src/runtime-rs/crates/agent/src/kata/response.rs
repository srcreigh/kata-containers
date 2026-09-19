// SPDX-License-Identifier: Apache-2.0
use anyhow::{ensure, Result};
pub(super) trait CheckedResponse {
    fn validate(&self) -> Result<()> {
        Ok(())
    }
}
impl CheckedResponse for crate::WaitProcessResponse {}
impl CheckedResponse for crate::WriteStreamResponse {}
impl CheckedResponse for crate::ReadStreamResponse {}
impl CheckedResponse for crate::MetricsResponse {}
impl CheckedResponse for crate::GetDiagnosticDataResponse {}
impl CheckedResponse for crate::OomEventResponse {
    fn validate(&self) -> Result<()> {
        ensure!(
            !self.container_id.is_empty() && self.container_id.len() <= 256,
            "invalid OOM identity length"
        );
        Ok(())
    }
}
impl CheckedResponse for crate::VolumeStatsResponse {
    fn validate(&self) -> Result<()> {
        ensure!(self.usage.len() <= 16, "excessive volume statistics");
        Ok(())
    }
}
impl CheckedResponse for crate::StatsContainerResponse {
    fn validate(&self) -> Result<()> {
        if let Some(stats) = self.cgroup_stats.as_ref() {
            if let Some(memory) = stats.memory_stats.as_ref() {
                ensure!(memory.stats.len() <= 4096, "excessive memory statistics");
            }
            ensure!(
                stats.hugetlb_stats.len() <= 256,
                "excessive hugepage statistics"
            );
            if let Some(block) = stats.blkio_stats.as_ref() {
                ensure!(
                    block.io_service_bytes_recursive.len() <= 4096,
                    "excessive block statistics"
                );
            }
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounds_repeated_statistics_and_event_ids() {
        let mut stats = crate::StatsContainerResponse::default();
        stats.validate().unwrap();
        stats.cgroup_stats = protobuf::MessageField::some(protocols::agent::CgroupStats {
            blkio_stats: protobuf::MessageField::some(protocols::agent::BlkioStats {
                io_service_bytes_recursive: vec![
                    protocols::agent::BlkioStatsEntry::default();
                    4097
                ],
                ..Default::default()
            }),
            ..Default::default()
        });
        assert!(stats.validate().is_err());
        assert!(crate::OomEventResponse {
            container_id: "x".repeat(257),
            ..Default::default()
        }
        .validate()
        .is_err());
    }
}
