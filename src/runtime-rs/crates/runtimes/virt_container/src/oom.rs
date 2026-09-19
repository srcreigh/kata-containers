// SPDX-License-Identifier: Apache-2.0
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

#[derive(Default)]
pub(crate) struct OomRegistry(RwLock<HashMap<String, Option<Instant>>>);
impl OomRegistry {
    pub async fn register(&self, id: &str) {
        self.0.write().await.insert(id.to_owned(), None);
    }
    pub async fn remove(&self, id: &str) {
        self.0.write().await.remove(id);
    }
    pub async fn accept(&self, id: &str) -> bool {
        if id.is_empty() || id.len() > 256 {
            return false;
        }
        let mut entries = self.0.write().await;
        let Some(last) = entries.get_mut(id) else {
            return false;
        };
        if last.is_some_and(|when| when.elapsed() < Duration::from_secs(1)) {
            return false;
        }
        *last = Some(Instant::now());
        true
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn only_live_local_ids_publish_and_duplicates_are_bounded() {
        let local = OomRegistry::default();
        let foreign = OomRegistry::default();
        local.register("one").await;
        foreign.register("two").await;
        assert!(!local.accept("two").await);
        assert!(!local.accept("../one").await);
        assert!(local.accept("one").await);
        assert!(!local.accept("one").await);
        local.remove("one").await;
        assert!(!local.accept("one").await);
        local.register("one").await;
        assert!(local.accept("one").await);
    }
}
