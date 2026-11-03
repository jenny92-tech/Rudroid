//! INPUT:  successful context/source/compile/link/binary lifecycle notifications
//! OUTPUT:  bounded per-share-group executable shader/program metadata
//! POS:    safe state machine; pending source never changes the current linked executable
#![forbid(unsafe_code)]
use std::collections::HashMap;
#[derive(Default, Clone, Copy)]
struct Shader {
    pending: bool,
    compiled: bool,
}
#[derive(Default)]
pub(super) struct State {
    groups: HashMap<usize, (u64, u64)>,
    next: u64,
    bindings: HashMap<u64, (u32, bool)>,
    shaders: HashMap<(u64, u32), Shader>,
    programs: HashMap<(u64, u32), bool>,
}
impl State {
    fn serial(&mut self) -> u64 {
        self.next = self.next.checked_add(1).expect("shader identity exhausted");
        self.next
    }
    pub fn group(&self, context: usize) -> u64 {
        self.groups.get(&context).map_or(0, |p| p.0)
    }
    pub fn context_group(&mut self, context: usize) -> u64 {
        if context == 0 { return 0; }
        self.ensure(context); self.group(context)
    }
    fn ensure(&mut self, context: usize) {
        if !self.groups.contains_key(&context) {
            let id = self.serial();
            self.groups.insert(context, (id, id));
        }
    }
    pub fn add_context(&mut self, context: usize, share: usize) {
        let group = if share == 0 {
            self.serial()
        } else {
            self.ensure(share);
            self.group(share)
        };
        let id = self.serial();
        self.groups.insert(context, (group, id));
    }
    pub fn binding(&mut self, context: usize, program: u32, install: bool) -> bool {
        self.ensure(context);
        let id = self.groups[&context].1;
        let previous = self.bindings.get(&id).copied();
        let previous = previous
            .filter(|(p, _)| *p == program)
            .map(|(_, skip)| skip);
        let skip = if program == 0 {
            false
        } else if install {
            self.program(context, program).or(previous).unwrap_or(false)
        } else {
            previous
                .or_else(|| self.program(context, program))
                .unwrap_or(false)
        };
        self.bindings.insert(id, (program, skip));
        skip
    }
    pub fn forget_context(&mut self, context: usize) -> Option<u64> {
        let group = self.group(context);
        if let Some((_, id)) = self.groups.remove(&context) {
            self.bindings.remove(&id);
        }
        if !self.groups.values().any(|g| g.0 == group) {
            self.shaders.retain(|(g, _), _| *g != group);
            self.programs.retain(|(g, _), _| *g != group);
            return (group != 0).then_some(group);
        }
        None
    }
    pub fn source(&mut self, context: usize, shader: u32, marked: bool) {
        self.ensure(context);
        let key = (self.group(context), shader);
        if self.shaders.len() < 4096 || self.shaders.contains_key(&key) {
            self.shaders.entry(key).or_default().pending = marked;
        }
    }
    pub fn compiled(&mut self, context: usize, shader: u32) {
        let key = (self.group(context), shader);
        if let Some(s) = self.shaders.get_mut(&key) {
            s.compiled = s.pending;
        }
    }
    pub fn forget_shader(&mut self, context: usize, shader: u32) {
        self.shaders.remove(&(self.group(context), shader));
    }
    pub fn shader_binary(&mut self, context: usize, shader: u32) {
        self.source(context, shader, false);
        self.compiled(context, shader);
    }
    pub fn linked(&mut self, context: usize, program: u32, attached: &[u32]) {
        self.ensure(context);
        let group = self.group(context);
        let skip = attached.iter().any(|shader| {
            self.shaders
                .get(&(group, *shader))
                .is_some_and(|s| s.compiled)
        });
        self.set_program(context, program, skip);
    }
    pub fn set_program(&mut self, context: usize, program: u32, skip: bool) {
        self.ensure(context);
        let group = self.group(context);
        let own = self.groups[&context].1;
        // Shared-object changes need not propagate immediately to peer contexts. A peer
        // forwards draws until an explicit bind, so either old or new executables render.
        for (g, id) in self.groups.values() {
            if *g == group && *id != own {
                if let Some((p, skip)) = self.bindings.get_mut(id) {
                    if *p == program {
                        *skip = false;
                    }
                }
            }
        }
        let key = (group, program);
        if self.programs.len() < 4096 || self.programs.contains_key(&key) {
            self.programs.insert(key, skip);
        }
    }
    pub fn program(&self, context: usize, program: u32) -> Option<bool> {
        self.programs.get(&(self.group(context), program)).copied()
    }
    pub fn delete_program(&mut self, context: usize, program: u32) {
        self.programs.remove(&(self.group(context), program));
    }
}
#[cfg(test)]
#[path = "../tests/unit/shader_state.rs"]
mod tests;
