use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use super::{OffsetPriority, PriceMode, RuntimeError, RuntimeResult, TargetPosConfig, VolumeSplitPolicy};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TaskId(u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegisteredTask {
    pub task_id: TaskId,
    pub created: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct TaskKey {
    runtime_id: String,
    account_key: String,
    symbol: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ConfigFingerprint {
    price_mode: PriceModeFingerprint,
    offset_priority: OffsetPriority,
    split_policy: Option<VolumeSplitPolicy>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum PriceModeFingerprint {
    Active,
    Passive,
    Custom(usize),
}

#[derive(Debug, Clone)]
struct TaskRegistration {
    task_id: TaskId,
    key: TaskKey,
    fingerprint: ConfigFingerprint,
}

#[derive(Debug, Default)]
struct RegistryState {
    next_task_id: u64,
    tasks_by_key: HashMap<TaskKey, TaskRegistration>,
    tasks_by_id: HashMap<TaskId, TaskRegistration>,
    order_owners: HashMap<String, TaskId>,
    task_orders: HashMap<TaskId, HashSet<String>>,
}

#[derive(Debug, Default)]
pub struct TaskRegistry {
    state: Mutex<RegistryState>,
}

impl TaskRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_target_task(
        &self,
        runtime_id: &str,
        account_key: &str,
        symbol: &str,
        config: &TargetPosConfig,
    ) -> RuntimeResult<RegisteredTask> {
        let key = TaskKey {
            runtime_id: runtime_id.to_string(),
            account_key: account_key.to_string(),
            symbol: symbol.to_string(),
        };
        let fingerprint = ConfigFingerprint::from(config);
        let mut state = self.state.lock().expect("task registry lock poisoned");

        if let Some(existing) = state.tasks_by_key.get(&key) {
            if existing.fingerprint == fingerprint {
                return Ok(RegisteredTask {
                    task_id: existing.task_id,
                    created: false,
                });
            }

            return Err(RuntimeError::TaskConflict {
                runtime_id: key.runtime_id,
                account_key: key.account_key,
                symbol: key.symbol,
            });
        }

        state.next_task_id += 1;
        let task_id = TaskId(state.next_task_id);
        let registration = TaskRegistration {
            task_id,
            key: key.clone(),
            fingerprint,
        };
        state.tasks_by_key.insert(key, registration.clone());
        state.tasks_by_id.insert(task_id, registration);

        Ok(RegisteredTask { task_id, created: true })
    }

    pub fn unregister_task(&self, task_id: TaskId) {
        let mut state = self.state.lock().expect("task registry lock poisoned");

        if let Some(registration) = state.tasks_by_id.remove(&task_id) {
            state.tasks_by_key.remove(&registration.key);
        }

        if let Some(order_ids) = state.task_orders.remove(&task_id) {
            for order_id in order_ids {
                state.order_owners.remove(&order_id);
            }
        }
    }

    pub fn bind_order_owner(&self, order_id: &str, task_id: TaskId) {
        let mut state = self.state.lock().expect("task registry lock poisoned");
        let order_id = order_id.to_string();

        if let Some(previous_owner) = state.order_owners.insert(order_id.clone(), task_id)
            && let Some(order_ids) = state.task_orders.get_mut(&previous_owner)
        {
            order_ids.remove(&order_id);
        }

        state.task_orders.entry(task_id).or_default().insert(order_id);
    }

    pub fn order_owner(&self, order_id: &str) -> Option<TaskId> {
        let state = self.state.lock().expect("task registry lock poisoned");
        state.order_owners.get(order_id).copied()
    }

    pub fn task_for_symbol(&self, runtime_id: &str, account_key: &str, symbol: &str) -> Option<TaskId> {
        let key = TaskKey {
            runtime_id: runtime_id.to_string(),
            account_key: account_key.to_string(),
            symbol: symbol.to_string(),
        };
        let state = self.state.lock().expect("task registry lock poisoned");
        state.tasks_by_key.get(&key).map(|registration| registration.task_id)
    }

    pub fn task_orders(&self, task_id: TaskId) -> Vec<String> {
        let state = self.state.lock().expect("task registry lock poisoned");
        state
            .task_orders
            .get(&task_id)
            .map(|order_ids| order_ids.iter().cloned().collect())
            .unwrap_or_default()
    }

    pub fn unbind_order_owner(&self, order_id: &str) {
        let mut state = self.state.lock().expect("task registry lock poisoned");
        let order_id = order_id.to_string();
        if let Some(task_id) = state.order_owners.remove(&order_id)
            && let Some(order_ids) = state.task_orders.get_mut(&task_id)
        {
            order_ids.remove(&order_id);
        }
    }
}

impl From<&TargetPosConfig> for ConfigFingerprint {
    fn from(value: &TargetPosConfig) -> Self {
        Self {
            price_mode: PriceModeFingerprint::from(&value.price_mode),
            offset_priority: value.offset_priority,
            split_policy: value.split_policy,
        }
    }
}

impl From<&PriceMode> for PriceModeFingerprint {
    fn from(value: &PriceMode) -> Self {
        match value {
            PriceMode::Active => Self::Active,
            PriceMode::Passive => Self::Passive,
            PriceMode::Custom(resolver) => Self::Custom(std::sync::Arc::as_ptr(resolver) as *const () as usize),
        }
    }
}
