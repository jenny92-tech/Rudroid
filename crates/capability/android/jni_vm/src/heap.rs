//! INPUT: JNI external roots, live object fields and array elements
//! OUTPUT: synchronized edge accounting and immediate collection of unreachable graphs
//! POS: session-owned heap operations follow liveness, objects, arrays lock order
#![forbid(unsafe_code)]
use super::{fields, objects, trim_reference_table, HashMap, RefTarget, Session};

impl Session {
    pub(super) fn pin_target(&self, target: RefTarget) {
        let (RefTarget::Str(id) | RefTarget::Throwable(id) | RefTarget::Object(id) | RefTarget::Array(id)) = target else { return; };
        let mut live = self.liveness.lock().unwrap_or_else(|p| p.into_inner());
        let count = live.entry(id).or_insert(0);
        *count = count.checked_add(1).expect("JNI entity pin count overflow");
    }

    /// Release exactly one reference, pending-exception owner or UTF borrow.
    /// Holding the count lock through removal prevents another pin racing
    /// with reclaim; no globals or other threads need to be scanned.
    pub(super) fn release_target(&self, target: RefTarget) { self.release_target_inner(target, false); }

    // Heap-to-heap edges are traced in place. Only external JNI roots enter
    // liveness for objects/arrays; immutable leaf storage remains pin-counted.
    pub(super) fn retain_edge(&self, target: RefTarget) {
        if matches!(target, RefTarget::Str(_) | RefTarget::Throwable(_)) { self.pin_target(target); return; }
        // Publication callers hold a source/temporary root. Serialize incoming
        // edge accounting with collection before publishing the owner's edge.
        let _live = self.liveness.lock().unwrap_or_else(|p| p.into_inner());
        match target {
            RefTarget::Object(id) => { let objects = self.objects.lock().unwrap(); let count = &objects[&id].incoming; count.set(count.get().checked_add(1).expect("heap edge overflow")); }
            RefTarget::Array(id) => { let arrays = self.arrays.lock().unwrap(); let count = &arrays[&id].incoming; count.set(count.get().checked_add(1).expect("heap edge overflow")); }
            _ => {}
        }
    }

    pub(super) fn release_target_inner(&self, target: RefTarget, edge: bool) {
        let mut live = self.liveness.lock().unwrap_or_else(|p| p.into_inner());
        self.release_locked(&mut live, target, edge);
    }

    pub(super) fn release_locked(&self, live: &mut HashMap<u64, usize>, target: RefTarget, edge: bool) {
        let (RefTarget::Str(id) | RefTarget::Throwable(id) | RefTarget::Object(id) | RefTarget::Array(id)) = target else { return; };
        if matches!(target, RefTarget::Object(_) | RefTarget::Array(_)) {
            if !edge {
                let count = live.get_mut(&id).expect("JNI external root released twice");
                *count = count.checked_sub(1).expect("JNI external root underflow");
            }
            if !edge && live.get(&id).copied().unwrap_or(0) != 0 { return; }
            let mut objects = self.objects.lock().unwrap();
            let mut arrays = self.arrays.lock().unwrap();
            if edge { Self::release_graph_edge(target, &objects, &arrays); }
            if live.get(&id).copied().unwrap_or(0) != 0 { return; }
            let graph_edge = |target| matches!(target, RefTarget::Object(_) | RefTarget::Array(_));
            let (incoming, has_children) = match target {
                RefTarget::Object(id) => { let object = &objects[&id]; (object.incoming.get(), object.fields.values().filter_map(|v|v.reference()).any(graph_edge)) }
                RefTarget::Array(id) => { let array = &arrays[&id]; (array.incoming.get(), array.elements.iter().flatten().copied().any(graph_edge)) }
                _ => unreachable!(),
            };
            if incoming == 0 {
                if self.reclaim_unreferenced_graph(target, live, &mut objects, &mut arrays) {
                    self.collect_objects(live, &mut objects, &mut arrays);
                }
                trim_reference_table(&mut objects); trim_reference_table(&mut arrays); trim_reference_table(live);
            } else if !has_children {
                // A node with no outgoing graph edges cannot be the sole path
                // to another owner. Removing its root therefore cannot make
                // any other graph node unreachable. Incoming heap edges keep
                // it alive; otherwise reclaim it immediately without tracing.
                // Its incoming owners remain reachable without this leaf.
            } else {
                self.collect_objects(live, &mut objects, &mut arrays);
            }
            return;
        }
        let count = live.get_mut(&id).expect("JNI leaf released without a pin");
        *count = count.checked_sub(1).expect("JNI leaf pin underflow");
        if *count == 0 { live.remove(&id); self.reclaim_leaf(target); trim_reference_table(live); }
    }

    fn reclaim_leaf(&self, target: RefTarget) {
        match target {
            RefTarget::Str(id) => {
                let mut strings = self.strings.lock().unwrap();
                strings.remove(&id); trim_reference_table(&mut strings);
            }
            RefTarget::Throwable(id) => {
                let mut throwables = self.throwables.lock().unwrap();
                throwables.remove(&id); trim_reference_table(&mut throwables);
            }
            RefTarget::Class(_) | RefTarget::ReflectedMethod(_) | RefTarget::ReflectedField(_) => {}
            _ => unreachable!("object/array requires graph collection"),
        }
    }

    fn release_graph_edge(target: RefTarget, objects: &HashMap<u64, fields::Object>, arrays: &HashMap<u64, objects::ObjectArray>) {
        let count = match target {
            RefTarget::Object(id) => &objects[&id].incoming,
            RefTarget::Array(id) => &arrays[&id].incoming,
            _ => return,
        };
        count.set(count.get().checked_sub(1).expect("heap edge underflow"));
    }

    fn release_heap_leaf(&self, live: &mut HashMap<u64, usize>, target: RefTarget) {
        let (RefTarget::Str(child) | RefTarget::Throwable(child)) = target else { return; };
        let count = live.get_mut(&child).expect("heap leaf edge is pinned");
        *count = count.checked_sub(1).expect("heap leaf edge underflow");
        if *count == 0 { live.remove(&child); self.reclaim_leaf(target); }
    }

    /// Peel only nodes proven unowned by both JNI roots and heap edges. A
    /// residual zero-root child with graph children may be part of a dead
    /// cycle, so request the ordinary full trace after the peel completes.
    fn reclaim_unreferenced_graph(&self, target: RefTarget, live: &mut HashMap<u64, usize>, objects: &mut HashMap<u64, fields::Object>, arrays: &mut HashMap<u64, objects::ObjectArray>) -> bool {
        let mut head = Some(target);
        let mut trace_required = false;
        while let Some(target) = head {
            match target {
                RefTarget::Object(id) => {
                    let object = objects.remove(&id).expect("zero-incoming object exists");
                    head = object.trace_next.take(); live.remove(&id);
                    for child in object.fields.into_values().filter_map(|v|v.reference()) { trace_required |= self.release_unreferenced_child(child, &mut head, live, objects, arrays); }
                }
                RefTarget::Array(id) => {
                    let array = arrays.remove(&id).expect("zero-incoming array exists");
                    head = array.trace_next.take(); live.remove(&id);
                    for child in array.elements.into_iter().flatten() { trace_required |= self.release_unreferenced_child(child, &mut head, live, objects, arrays); }
                }
                _ => unreachable!("only graph nodes enter the reclamation list"),
            }
        }
        trace_required
    }

    fn release_unreferenced_child(&self, child: RefTarget, head: &mut Option<RefTarget>, live: &mut HashMap<u64, usize>, objects: &HashMap<u64, fields::Object>, arrays: &HashMap<u64, objects::ObjectArray>) -> bool {
        Self::release_graph_edge(child, objects, arrays);
        self.release_heap_leaf(live, child);
        let (id, incoming, next) = match child {
            RefTarget::Object(id) => { let object = &objects[&id]; (id, &object.incoming, &object.trace_next) }
            RefTarget::Array(id) => { let array = &arrays[&id]; (id, &array.incoming, &array.trace_next) }
            _ => return false,
        };
        if live.get(&id).copied().unwrap_or(0) != 0 { return false; }
        if incoming.get() == 0 { next.set(*head); *head = Some(child); return false; }
        match child {
            RefTarget::Object(id) => objects[&id].fields.values().filter_map(|v|v.reference()).any(|t|matches!(t,RefTarget::Object(_)|RefTarget::Array(_))),
            RefTarget::Array(id) => arrays[&id].elements.iter().flatten().any(|t|matches!(t,RefTarget::Object(_)|RefTarget::Array(_))),
            _ => unreachable!(),
        }
    }

    pub(super) fn collect_objects(&self, live: &mut HashMap<u64, usize>, objects: &mut HashMap<u64, fields::Object>, arrays: &mut HashMap<u64, objects::ObjectArray>) -> usize {
        for object in objects.values() { object.marked.set(false); object.trace_next.set(None); }
        for array in arrays.values() { array.marked.set(false); array.trace_next.set(None); }
        // Intrusive work list: each reachable node is expanded once, including
        // shared children and cycles. No allocation or recursive Rust stack;
        // all scratch links stay protected by the same collector locks.
        let mark = |target, head: &mut Option<RefTarget>| {
            let (marked, next) = match target {
                RefTarget::Object(id) => { let object = objects.get(&id).expect("live object edge"); (&object.marked, &object.trace_next) }
                RefTarget::Array(id) => { let array = arrays.get(&id).expect("live array edge"); (&array.marked, &array.trace_next) }
                _ => return,
            };
            if !marked.replace(true) { next.set(*head); *head = Some(target); }
        };
        let mut head = None;
        for id in objects.keys().filter(|id| live.get(id).copied().unwrap_or(0) != 0) { mark(RefTarget::Object(*id), &mut head); }
        for id in arrays.keys().filter(|id| live.get(id).copied().unwrap_or(0) != 0) { mark(RefTarget::Array(*id), &mut head); }
        let mut expanded = 0;
        while let Some(target) = head {
            expanded += 1;
            match target {
                RefTarget::Object(id) => {
                    let object = &objects[&id]; head = object.trace_next.take();
                    for target in object.fields.values().filter_map(|v| v.reference()) { mark(target, &mut head); }
                }
                RefTarget::Array(id) => {
                    let array = &arrays[&id]; head = array.trace_next.take();
                    for target in array.elements.iter().flatten() { mark(*target, &mut head); }
                }
                _ => unreachable!("only graph nodes enter the trace list"),
            }
        }
        // All dead owners are still present while their outgoing leaves are
        // released. No recursive collector or in-flight edge counters exist.
        for object in objects.values().filter(|o| !o.marked.get()) {
            for target in object.fields.values().filter_map(|v| v.reference()) { Self::release_graph_edge(target, objects, arrays); self.release_heap_leaf(live,target); }
        }
        for array in arrays.values().filter(|a| !a.marked.get()) {
            for target in array.elements.iter().flatten() { Self::release_graph_edge(*target, objects, arrays); self.release_heap_leaf(live,*target); }
        }
        objects.retain(|id,object| { if object.marked.get() { true } else { live.remove(id); false } });
        arrays.retain(|id,array| { if array.marked.get() { true } else { live.remove(id); false } });
        trim_reference_table(objects); trim_reference_table(arrays); trim_reference_table(live);
        expanded
    }

    // -- class / method resolution
}
