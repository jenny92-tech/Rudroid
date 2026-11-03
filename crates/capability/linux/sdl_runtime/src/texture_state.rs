//! INPUT:  unique GL share-group identity, successful allocations and deletion notifications
//! OUTPUT:  bounded texture plans scoped to native shared-object namespaces
//! POS:    safe metadata; holds no GPU ownership, pointers, locks across driver calls
#![forbid(unsafe_code)]
use super::texture_policy::{Param, Plan};
use std::collections::{HashMap, HashSet};
#[derive(Default)]
pub(super) struct State {
    plans: HashMap<(u64, u32), Record>,
    reserved: HashSet<(u64, u32)>,
}
struct Record {
    plan: Plan,
    parameters: Option<[Param; 4]>,
}
impl State {
    pub fn room(&self, key: (u64, u32)) -> bool {
        key.0 != 0
            && key.1 != 0
            && (self.plans.len() + self.reserved.len() < 8192 || self.plans.contains_key(&key))
    }
    pub fn reserve(&mut self, key: (u64, u32)) -> bool {
        if !self.room(key) || self.reserved.contains(&key) {
            return false;
        }
        if self.reserved.try_reserve(1).is_err()
            || self.plans.try_reserve(self.reserved.len() + 1).is_err()
        {
            return false;
        }
        self.reserved.insert(key);
        true
    }
    pub fn release(&mut self, key: (u64, u32)) {
        self.reserved.remove(&key);
    }
    pub fn get(&self, key: (u64, u32)) -> Option<Plan> {
        self.plans.get(&key).map(|r| r.plan)
    }
    pub fn parameters(&self, key: (u64, u32)) -> Option<[Param; 4]> {
        self.plans.get(&key)?.parameters
    }
    pub fn set_parameters(&mut self, key: (u64, u32), parameters: [Param; 4]) {
        if let Some(r) = self.plans.get_mut(&key) {
            r.parameters = Some(parameters);
        }
    }
    pub fn set(&mut self, key: (u64, u32), plan: Option<Plan>) {
        match plan {
            Some(p) if self.room(key) => {
                self.plans.insert(
                    key,
                    Record {
                        plan: p,
                        parameters: None,
                    },
                );
            }
            Some(_) => {}
            None => {
                self.plans.remove(&key);
            }
        }
    }
    pub fn retire(&mut self, group: u64) {
        self.plans.retain(|(g, _), _| *g != group);
        self.reserved.retain(|(g, _)| *g != group);
    }
}
#[cfg(test)]
#[path = "../tests/unit/texture_state.rs"]
mod tests;
