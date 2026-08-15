use std::collections::HashMap;
use std::sync::{Arc, Weak};

use tokio::sync::{Mutex, OwnedMutexGuard};

#[derive(Default)]
pub struct ClientMarketActionLocks {
    locks: Mutex<HashMap<String, Weak<Mutex<()>>>>,
}

impl ClientMarketActionLocks {
    pub(crate) async fn lock(&self, installation_id: &str) -> OwnedMutexGuard<()> {
        let lock = {
            let mut locks = self.locks.lock().await;
            locks.retain(|_, lock| lock.strong_count() > 0);
            match locks.get(installation_id).and_then(Weak::upgrade) {
                Some(lock) => lock,
                None => {
                    let lock = Arc::new(Mutex::new(()));
                    locks.insert(installation_id.to_string(), Arc::downgrade(&lock));
                    lock
                }
            }
        };
        lock.lock_owned().await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use super::*;

    #[tokio::test]
    async fn serializes_actions_for_the_same_installation_only() {
        let locks = Arc::new(ClientMarketActionLocks::default());
        let first = locks.lock("installation-a").await;

        let same_locks = locks.clone();
        let same = tokio::spawn(async move { same_locks.lock("installation-a").await });
        let other_locks = locks.clone();
        let other = tokio::spawn(async move { other_locks.lock("installation-b").await });

        assert!(
            tokio::time::timeout(Duration::from_millis(100), other)
                .await
                .expect("different installation must not block")
                .is_ok()
        );
        assert!(!same.is_finished());

        drop(first);
        assert!(
            tokio::time::timeout(Duration::from_millis(100), same)
                .await
                .expect("same installation must continue after unlock")
                .is_ok()
        );
    }
}
