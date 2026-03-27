//! Call-graph analysis and static memory allocation for the "global mode"
//! calling convention.
//!
//! The Intel 8080 has no efficient stack-frame mechanism (no base-pointer
//! register, no indexed addressing).  The key optimisation is to assign every
//! **non-recursive** function's locals and parameters to fixed RAM addresses at
//! compile time.  Recursive functions fall back to stack mode.
//!
//! # Algorithm
//!
//! 1. Build the static call graph by scanning each function's IR for `Call`
//!    instructions.
//! 2. Detect cycles (recursive / mutually-recursive functions) using DFS with
//!    white / gray / black colouring.
//! 3. Assign fixed RAM addresses:
//!    a. Global variables are laid out first, starting at `base_addr`.
//!    b. Each non-recursive function's parameters and locals get unique
//!       addresses after the globals.
//!    c. Recursive functions are marked as `stack_mode`.
//!
//! Phase 1 simplification: every non-recursive function gets its own
//! non-overlapping address range (no call-tree–based sharing yet).

use std::collections::{HashMap, HashSet};

use crate::ir::{IrFunction, IrOp, IrProgram};
use crate::regalloc::PhysReg;
use crate::types::CType;

/// Default base address for variable allocation (upper RAM on Вектор-06Ц).
pub const DEFAULT_BASE_ADDR: u16 = 0x8000;

// ---------------------------------------------------------------------------
// CallGraph
// ---------------------------------------------------------------------------

/// The static call graph extracted from an [`IrProgram`].
#[derive(Debug, Clone)]
pub struct CallGraph {
    /// Maps function name → set of functions it directly calls.
    pub callees: HashMap<String, HashSet<String>>,
    /// Functions that participate in a cycle (direct or mutual recursion).
    pub recursive: HashSet<String>,
    /// Functions that must use the stack calling convention.
    pub stack_mode: HashSet<String>,
}

// ---------------------------------------------------------------------------
// CallGraphAnalysis — the public result
// ---------------------------------------------------------------------------

/// Complete result of call-graph analysis for one translation unit.
#[derive(Debug, Clone)]
pub struct CallGraphAnalysis {
    /// The call graph itself.
    pub graph: CallGraph,

    /// Static allocation for local variables and parameters of non-recursive
    /// functions.
    ///
    /// Key: label of the form `_l_<func>_<var>`.
    /// Value: fixed RAM address.
    pub local_allocs: HashMap<String, u16>,

    /// Static allocation for global variables.
    ///
    /// Key: label of the form `_g_<var>`.
    /// Value: fixed RAM address.
    pub global_allocs: HashMap<String, u16>,

    /// First address past all allocations — useful for placing the heap or
    /// stack pointer.
    pub next_addr: u16,

    /// Leaf functions (functions that make no calls).
    /// These can skip register save/restore.
    pub leaf_functions: HashSet<String>,

    /// Interprocedural effect summaries for known and user-defined functions.
    pub effects: HashMap<String, FunctionEffects>,
}

/// A conservative interprocedural summary for one function.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionEffects {
    /// The function does not call any other function.
    pub leaf: bool,
    /// The function has no externally visible side effects and does not read
    /// mutable non-local memory.
    pub pure: bool,
    /// The function does not write externally visible memory.
    pub readonly: bool,
    /// The function never returns to its caller.
    pub noreturn: bool,
    /// Registers whose contents are not preserved across the call.
    pub clobbers: HashSet<PhysReg>,
}

impl FunctionEffects {
    fn conservative() -> Self {
        Self {
            leaf: false,
            pure: false,
            readonly: false,
            noreturn: false,
            clobbers: all_call_clobbers(),
        }
    }

    fn runtime_helper() -> Self {
        Self {
            leaf: true,
            pure: true,
            readonly: true,
            noreturn: false,
            clobbers: all_call_clobbers(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct DirectEffects {
    reads_nonlocal_memory: bool,
    writes_nonlocal_memory: bool,
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Analyse an [`IrProgram`] and produce static memory allocations.
///
/// `base_addr` is the first RAM address available for variables (typically
/// after the code segment).  Pass `None` to use [`DEFAULT_BASE_ADDR`].
pub fn analyze(program: &IrProgram, base_addr: Option<u16>) -> CallGraphAnalysis {
    let base = base_addr.unwrap_or(DEFAULT_BASE_ADDR);

    // 1. Build call graph edges.
    let callees = build_callees(program);

    // 2. Detect recursive functions.
    let recursive = detect_recursion(&callees);

    // 3. Stack-mode = recursive functions.
    let stack_mode: HashSet<String> = recursive.clone();

    // 4. Identify leaf functions (those that call nothing).
    let leaf_functions: HashSet<String> = callees
        .iter()
        .filter(|(_, targets)| targets.is_empty())
        .map(|(name, _)| name.clone())
        .collect();

    let graph = CallGraph {
        callees,
        recursive,
        stack_mode,
    };

    // 5. Allocate addresses.
    let (global_allocs, addr_after_globals) = allocate_globals(program, base);
    let (local_allocs, next_addr) =
        allocate_locals(program, &graph, addr_after_globals);
    let effects = summarize_functions(program, &graph, &leaf_functions);

    CallGraphAnalysis {
        graph,
        local_allocs,
        global_allocs,
        next_addr,
        leaf_functions,
        effects,
    }
}

// ---------------------------------------------------------------------------
// Step 1 — build callee map
// ---------------------------------------------------------------------------

fn build_callees(program: &IrProgram) -> HashMap<String, HashSet<String>> {
    let mut map: HashMap<String, HashSet<String>> = HashMap::new();

    // Ensure every defined function has an entry even if it calls nothing.
    for func in &program.functions {
        map.entry(func.name.clone()).or_default();
    }

    for func in &program.functions {
        for instr in &func.body {
            if let IrOp::Call { func_name, .. } = &instr.op {
                map.entry(func.name.clone())
                    .or_default()
                    .insert(func_name.clone());
            }
        }
    }

    map
}

// ---------------------------------------------------------------------------
// Step 2 — cycle detection (DFS with colouring)
// ---------------------------------------------------------------------------

/// Node colour during DFS.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Colour {
    White, // unvisited
    Gray,  // on the current DFS path
    Black, // finished
}

fn detect_recursion(callees: &HashMap<String, HashSet<String>>) -> HashSet<String> {
    let mut colour: HashMap<&str, Colour> = HashMap::new();
    for name in callees.keys() {
        colour.insert(name.as_str(), Colour::White);
    }

    let mut recursive = HashSet::new();
    // Ancestor stack for the current DFS path — used to mark all functions on
    // the cycle, not just the back-edge target.
    let mut path: Vec<String> = Vec::new();

    for name in callees.keys() {
        if colour[name.as_str()] == Colour::White {
            dfs_visit(name, callees, &mut colour, &mut recursive, &mut path);
        }
    }

    recursive
}

fn dfs_visit<'a>(
    node: &str,
    callees: &'a HashMap<String, HashSet<String>>,
    colour: &mut HashMap<&'a str, Colour>,
    recursive: &mut HashSet<String>,
    path: &mut Vec<String>,
) {
    // Only visit nodes that are defined in the program (have an entry in the
    // callee map).  Calls to external / library functions are ignored.
    let Some(neighbours) = callees.get(node) else {
        return;
    };

    if let Some(c) = colour.get(node) {
        if *c != Colour::White {
            return;
        }
    } else {
        return;
    }

    // Safety: `node` is a key in `callees` so it was inserted in `colour`
    // during initialisation.  We need the &'a str that is stored in the
    // HashMap key — retrieve it here.
    let node_ref: &'a str = callees
        .get_key_value(node)
        .expect("node must be present")
        .0
        .as_str();

    colour.insert(node_ref, Colour::Gray);
    path.push(node.to_owned());

    for callee in neighbours {
        match colour.get(callee.as_str()) {
            Some(Colour::Gray) => {
                // Back edge — mark every function on the path from the callee
                // back to the current node (inclusive) as recursive.
                let mut found = false;
                for ancestor in path.iter() {
                    if ancestor == callee {
                        found = true;
                    }
                    if found {
                        recursive.insert(ancestor.clone());
                    }
                }
            }
            Some(Colour::White) | None => {
                dfs_visit(callee, callees, colour, recursive, path);
            }
            Some(Colour::Black) => { /* already fully processed */ }
        }
    }

    path.pop();
    colour.insert(node_ref, Colour::Black);
}

// ---------------------------------------------------------------------------
// Step 3a — allocate globals
// ---------------------------------------------------------------------------

fn allocate_globals(program: &IrProgram, base: u16) -> (HashMap<String, u16>, u16) {
    let mut allocs = HashMap::new();
    let mut addr = base;

    for gvar in &program.globals {
        let label = if gvar.name.starts_with('_') {
            gvar.name.clone()
        } else {
            format!("_g_{}", gvar.name)
        };
        allocs.insert(label, addr);

        let size = gvar.ty.size_of().unwrap_or(2) as u16;
        addr = addr.wrapping_add(size);
    }

    (allocs, addr)
}

// ---------------------------------------------------------------------------
// Step 3b — allocate locals / parameters for non-recursive functions
// ---------------------------------------------------------------------------

fn allocate_locals(
    program: &IrProgram,
    graph: &CallGraph,
    start_addr: u16,
) -> (HashMap<String, u16>, u16) {
    let mut allocs = HashMap::new();
    let frame_layouts = build_frame_layouts(program, graph);
    let reachability = compute_reachability(graph);
    let frame_bases = allocate_frame_bases(&frame_layouts, &reachability, start_addr);

    for func in &program.functions {
        let Some(layout) = frame_layouts.get(&func.name) else {
            continue;
        };
        let Some(&frame_base) = frame_bases.get(&func.name) else {
            continue;
        };

        for param in &func.params {
            let label = format!("_l_{}_{}", func.name, param.name);
            if let Some(&offset) = layout.offsets.get(&label) {
                allocs.insert(label, frame_base.wrapping_add(offset));
            }
        }

        for (name, _ty, _offset) in &func.locals {
            let label = format!("_l_{}_{}", func.name, name);
            if let Some(&offset) = layout.offsets.get(&label) {
                allocs.insert(label, frame_base.wrapping_add(offset));
            }
        }
    }

    let next_addr = frame_bases
        .iter()
        .filter_map(|(name, &base)| frame_layouts.get(name).map(|layout| base + layout.size))
        .max()
        .unwrap_or(start_addr);

    (allocs, next_addr)
}

/// Byte size used for a parameter in static allocation.
fn param_size(ty: &CType) -> u16 {
    ty.size_of().unwrap_or(2) as u16
}

/// Byte size used for a local variable in static allocation.
fn var_size(ty: &CType) -> u16 {
    ty.size_of().unwrap_or(2) as u16
}

#[derive(Debug, Clone, Default)]
struct FunctionFrameLayout {
    size: u16,
    offsets: HashMap<String, u16>,
}

fn build_frame_layouts(
    program: &IrProgram,
    graph: &CallGraph,
) -> HashMap<String, FunctionFrameLayout> {
    let mut layouts = HashMap::new();

    for func in &program.functions {
        if graph.stack_mode.contains(&func.name) {
            continue;
        }

        let mut offsets = HashMap::new();
        let mut next_offset = 0u16;

        for param in &func.params {
            let label = format!("_l_{}_{}", func.name, param.name);
            offsets.insert(label, next_offset);
            next_offset = next_offset.wrapping_add(param_size(&param.ty));
        }

        for (name, ty, _offset) in &func.locals {
            let label = format!("_l_{}_{}", func.name, name);
            if offsets.contains_key(&label) {
                continue;
            }
            offsets.insert(label, next_offset);
            next_offset = next_offset.wrapping_add(var_size(ty));
        }

        layouts.insert(
            func.name.clone(),
            FunctionFrameLayout {
                size: next_offset,
                offsets,
            },
        );
    }

    layouts
}

fn compute_reachability(graph: &CallGraph) -> HashMap<String, HashSet<String>> {
    let defined: HashSet<String> = graph.callees.keys().cloned().collect();
    let mut reachability = HashMap::new();

    for func in graph.callees.keys() {
        let mut seen = HashSet::new();
        let mut stack: Vec<String> = graph
            .callees
            .get(func)
            .into_iter()
            .flat_map(|callees| callees.iter().cloned())
            .filter(|callee| defined.contains(callee))
            .collect();

        while let Some(node) = stack.pop() {
            if !seen.insert(node.clone()) {
                continue;
            }
            if let Some(next) = graph.callees.get(&node) {
                for callee in next {
                    if defined.contains(callee) {
                        stack.push(callee.clone());
                    }
                }
            }
        }

        reachability.insert(func.clone(), seen);
    }

    reachability
}

fn allocate_frame_bases(
    layouts: &HashMap<String, FunctionFrameLayout>,
    reachability: &HashMap<String, HashSet<String>>,
    start_addr: u16,
) -> HashMap<String, u16> {
    let mut ordered: Vec<(&String, &FunctionFrameLayout)> = layouts.iter().collect();
    ordered.sort_by(|(name_a, layout_a), (name_b, layout_b)| {
        layout_b
            .size
            .cmp(&layout_a.size)
            .then_with(|| name_a.cmp(name_b))
    });

    let mut frame_bases = HashMap::new();
    let mut placed: Vec<(String, u16, u16)> = Vec::new();

    for (name, layout) in ordered {
        if layout.size == 0 {
            frame_bases.insert(name.clone(), start_addr);
            continue;
        }

        let mut candidate_bases = vec![start_addr];
        for (other_name, other_base, other_size) in &placed {
            if functions_overlap_lifetime(name, other_name, reachability) {
                candidate_bases.push(other_base.wrapping_add(*other_size));
            }
        }
        candidate_bases.sort_unstable();
        candidate_bases.dedup();

        let base = candidate_bases
            .into_iter()
            .find(|candidate| {
                placed.iter().all(|(other_name, other_base, other_size)| {
                    if !functions_overlap_lifetime(name, other_name, reachability) {
                        return true;
                    }
                    let candidate_end = candidate.wrapping_add(layout.size);
                    let other_end = other_base.wrapping_add(*other_size);
                    candidate_end <= *other_base || other_end <= *candidate
                })
            })
            .unwrap_or(start_addr);

        frame_bases.insert(name.clone(), base);
        placed.push((name.clone(), base, layout.size));
    }

    frame_bases
}

fn functions_overlap_lifetime(
    left: &str,
    right: &str,
    reachability: &HashMap<String, HashSet<String>>,
) -> bool {
    reachability
        .get(left)
        .is_some_and(|seen| seen.contains(right))
        || reachability
            .get(right)
            .is_some_and(|seen| seen.contains(left))
        || left == right
}

fn summarize_functions(
    program: &IrProgram,
    graph: &CallGraph,
    leaf_functions: &HashSet<String>,
) -> HashMap<String, FunctionEffects> {
    let direct_effects: HashMap<String, DirectEffects> = program
        .functions
        .iter()
        .map(|func| (func.name.clone(), scan_direct_effects(func)))
        .collect();

    let mut summaries = known_function_effects();

    for func in &program.functions {
        summaries.insert(
            func.name.clone(),
            FunctionEffects {
                leaf: leaf_functions.contains(&func.name),
                pure: true,
                readonly: true,
                noreturn: false,
                clobbers: all_call_clobbers(),
            },
        );
    }

    for _ in 0..program.functions.len().max(1) {
        let mut changed = false;
        for func in &program.functions {
            let direct = direct_effects.get(&func.name).copied().unwrap_or_default();
            let mut summary = summaries
                .get(&func.name)
                .cloned()
                .unwrap_or_else(FunctionEffects::conservative);

            summary.leaf = leaf_functions.contains(&func.name);
            summary.readonly = !direct.writes_nonlocal_memory;
            summary.pure = !direct.writes_nonlocal_memory && !direct.reads_nonlocal_memory;

            if let Some(callees) = graph.callees.get(&func.name) {
                for callee in callees {
                    let callee_summary = summaries
                        .get(callee)
                        .cloned()
                        .unwrap_or_else(FunctionEffects::conservative);
                    if !callee_summary.readonly {
                        summary.readonly = false;
                    }
                    if !callee_summary.pure {
                        summary.pure = false;
                    }
                    if callee_summary.noreturn {
                        summary.noreturn = true;
                    }
                }
            }

            let current = summaries.get(&func.name).cloned();
            if current.as_ref() != Some(&summary) {
                summaries.insert(func.name.clone(), summary);
                changed = true;
            }
        }

        if !changed {
            break;
        }
    }

    summaries
}

fn scan_direct_effects(func: &IrFunction) -> DirectEffects {
    let mut effects = DirectEffects::default();
    let local_prefix = format!("_l_{}", func.name);

    for instr in &func.body {
        match &instr.op {
            IrOp::LoadGlobal { addr_label, .. } => {
                if !addr_label.starts_with(&local_prefix) {
                    effects.reads_nonlocal_memory = true;
                }
            }
            IrOp::StoreGlobal { addr_label, .. } => {
                if !addr_label.starts_with(&local_prefix) {
                    effects.writes_nonlocal_memory = true;
                }
            }
            IrOp::LoadPtr { .. } => {
                effects.reads_nonlocal_memory = true;
            }
            IrOp::StorePtr { .. } | IrOp::StoreLocal { .. } => {
                effects.writes_nonlocal_memory = true;
            }
            IrOp::LoadLocal { .. } => {}
            IrOp::Call { .. } => {}
            _ => {}
        }
    }

    effects
}

fn all_call_clobbers() -> HashSet<PhysReg> {
    HashSet::from([PhysReg::A, PhysReg::BC, PhysReg::DE, PhysReg::HL])
}

fn known_function_effects() -> HashMap<String, FunctionEffects> {
    let mut effects = HashMap::new();
    for name in [
        "__mul16", "__mul32",
        "__div16u", "__div16s", "__mod16u", "__mod16s", "__divmod16u",
        "__div32u", "__div32s", "__mod32u", "__mod32s", "__divmod32u",
        "__shl16", "__shr16u", "__shr16s", "__shl32", "__shr32u", "__shr32s",
        "__cmp16u", "__cmp16s",
        "__fadd", "__fsub", "__fmul", "__fdiv",
        "__feq", "__fne", "__flt", "__fle", "__fgt", "__fge",
        "__itof", "__ftoi",
        "__builtin_va_start",
    ] {
        effects.insert(name.to_string(), FunctionEffects::runtime_helper());
    }
    effects
}

// ---------------------------------------------------------------------------
// Convenience queries on CallGraphAnalysis
// ---------------------------------------------------------------------------

impl CallGraphAnalysis {
    /// Returns `true` if `func_name` should use the stack calling convention.
    pub fn is_stack_mode(&self, func_name: &str) -> bool {
        self.graph.stack_mode.contains(func_name)
    }

    /// Returns `true` if `func_name` participates in a recursive cycle.
    pub fn is_recursive(&self, func_name: &str) -> bool {
        self.graph.recursive.contains(func_name)
    }

    /// Look up the fixed RAM address for a local / parameter.
    ///
    /// `func` is the function name and `var` is the variable name.
    pub fn local_addr(&self, func: &str, var: &str) -> Option<u16> {
        let label = format!("_l_{}_{}", func, var);
        self.local_allocs.get(&label).copied()
    }

    /// Look up the fixed RAM address for a global variable.
    pub fn global_addr(&self, var: &str) -> Option<u16> {
        let label = format!("_g_{}", var);
        self.global_allocs.get(&label).copied()
    }

    /// Returns `true` if the function is a leaf (makes no calls).
    pub fn is_leaf(&self, func_name: &str) -> bool {
        self.leaf_functions.contains(func_name)
    }

    /// Return the effect summary for a function or a conservative fallback.
    pub fn effects_for(&self, func_name: &str) -> FunctionEffects {
        self.effects
            .get(func_name)
            .cloned()
            .unwrap_or_else(FunctionEffects::conservative)
    }
}

impl CallGraph {
    /// Returns the set of functions directly called by `func_name`.
    pub fn callees_of(&self, func_name: &str) -> Option<&HashSet<String>> {
        self.callees.get(func_name)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{IrFunction, IrOp, IrParam, IrProgram, GlobalVar, VReg, Width};
    use crate::types::CType;

    // -- helpers ----------------------------------------------------------

    /// Create a minimal function that calls a list of other functions.
    fn make_func(name: &str, calls: &[&str]) -> IrFunction {
        let mut f = IrFunction::new(name, CType::Void);
        for callee in calls {
            f.push_op(IrOp::call(*callee, vec![], None));
        }
        f
    }

    /// Create a function with locals and parameters.
    fn make_func_with_vars(
        name: &str,
        params: &[(&str, CType)],
        locals: &[(&str, CType)],
        calls: &[&str],
    ) -> IrFunction {
        let mut f = IrFunction::new(name, CType::int_signed());
        for (i, (pname, ty)) in params.iter().enumerate() {
            let w = Width::from_ctype(ty).unwrap_or(Width::W16);
            f.params.push(IrParam {
                name: (*pname).to_string(),
                ty: ty.clone(),
                vreg: VReg::new(i as u32, w),
            });
        }
        let mut offset: i32 = 0;
        for (lname, ty) in locals {
            let sz = ty.size_of().unwrap_or(2) as i32;
            f.locals.push(((*lname).to_string(), ty.clone(), offset));
            offset += sz;
        }
        for callee in calls {
            f.push_op(IrOp::call(*callee, vec![], None));
        }
        f
    }

    fn make_program(functions: Vec<IrFunction>) -> IrProgram {
        IrProgram {
            globals: Vec::new(),
            functions,
            strings: Vec::new(),
        }
    }

    // -- call graph construction -----------------------------------------

    #[test]
    fn empty_program() {
        let prog = make_program(vec![]);
        let result = analyze(&prog, None);
        assert!(result.graph.callees.is_empty());
        assert!(result.graph.recursive.is_empty());
        assert!(result.local_allocs.is_empty());
        assert!(result.global_allocs.is_empty());
        assert_eq!(result.next_addr, DEFAULT_BASE_ADDR);
    }

    #[test]
    fn single_function_no_calls() {
        let prog = make_program(vec![make_func("main", &[])]);
        let result = analyze(&prog, None);

        assert_eq!(result.graph.callees.len(), 1);
        assert!(result.graph.callees["main"].is_empty());
        assert!(!result.is_recursive("main"));
        assert!(!result.is_stack_mode("main"));
    }

    #[test]
    fn linear_call_chain() {
        // main -> foo -> bar
        let prog = make_program(vec![
            make_func("main", &["foo"]),
            make_func("foo", &["bar"]),
            make_func("bar", &[]),
        ]);
        let result = analyze(&prog, None);

        assert!(result.graph.callees["main"].contains("foo"));
        assert!(result.graph.callees["foo"].contains("bar"));
        assert!(result.graph.callees["bar"].is_empty());
        assert!(!result.is_recursive("main"));
        assert!(!result.is_recursive("foo"));
        assert!(!result.is_recursive("bar"));
    }

    #[test]
    fn call_to_external_function() {
        // main calls "printf" which is not defined in the program.
        let prog = make_program(vec![make_func("main", &["printf"])]);
        let result = analyze(&prog, None);

        assert!(result.graph.callees["main"].contains("printf"));
        assert!(!result.is_recursive("main"));
    }

    // -- recursion detection ---------------------------------------------

    #[test]
    fn direct_recursion() {
        // f calls itself
        let prog = make_program(vec![make_func("f", &["f"])]);
        let result = analyze(&prog, None);
        assert!(result.is_recursive("f"));
        assert!(result.is_stack_mode("f"));
    }

    #[test]
    fn mutual_recursion() {
        // a -> b -> a
        let prog = make_program(vec![
            make_func("a", &["b"]),
            make_func("b", &["a"]),
        ]);
        let result = analyze(&prog, None);
        assert!(result.is_recursive("a"));
        assert!(result.is_recursive("b"));
    }

    #[test]
    fn partial_recursion() {
        // main -> a -> b -> a  (cycle in a,b; main is NOT recursive)
        let prog = make_program(vec![
            make_func("main", &["a"]),
            make_func("a", &["b"]),
            make_func("b", &["a"]),
        ]);
        let result = analyze(&prog, None);
        assert!(!result.is_recursive("main"));
        assert!(result.is_recursive("a"));
        assert!(result.is_recursive("b"));
    }

    #[test]
    fn diamond_no_recursion() {
        // main -> a, main -> b, a -> c, b -> c
        let prog = make_program(vec![
            make_func("main", &["a", "b"]),
            make_func("a", &["c"]),
            make_func("b", &["c"]),
            make_func("c", &[]),
        ]);
        let result = analyze(&prog, None);
        assert!(!result.is_recursive("main"));
        assert!(!result.is_recursive("a"));
        assert!(!result.is_recursive("b"));
        assert!(!result.is_recursive("c"));
    }

    // -- global allocation -----------------------------------------------

    #[test]
    fn global_allocation_basic() {
        let prog = IrProgram {
            globals: vec![
                GlobalVar {
                    name: "x".into(),
                    ty: CType::int_signed(),
                    init: None,
                },
                GlobalVar {
                    name: "buf".into(),
                    ty: CType::array(CType::char_signed(), 10),
                    init: None,
                },
            ],
            functions: vec![],
            strings: vec![],
        };

        let result = analyze(&prog, Some(0x8000));

        assert_eq!(result.global_addr("x"), Some(0x8000));
        // int = 2 bytes, so buf starts at 0x8002
        assert_eq!(result.global_addr("buf"), Some(0x8002));
        // buf = 10 * 1 = 10 bytes, next addr = 0x800C
        assert_eq!(result.next_addr, 0x800C);
    }

    // -- local allocation ------------------------------------------------

    #[test]
    fn local_allocation_simple() {
        let prog = make_program(vec![make_func_with_vars(
            "foo",
            &[("a", CType::int_signed())],
            &[("x", CType::int_signed()), ("y", CType::char_signed())],
            &[],
        )]);

        let result = analyze(&prog, Some(0x8000));

        // param a: 2 bytes at 0x8000
        assert_eq!(result.local_addr("foo", "a"), Some(0x8000));
        // local x: 2 bytes at 0x8002
        assert_eq!(result.local_addr("foo", "x"), Some(0x8002));
        // local y: 1 byte at 0x8004
        assert_eq!(result.local_addr("foo", "y"), Some(0x8004));
        // next: 0x8005
        assert_eq!(result.next_addr, 0x8005);
    }

    #[test]
    fn recursive_function_gets_no_static_locals() {
        let prog = make_program(vec![make_func_with_vars(
            "rec",
            &[("n", CType::int_signed())],
            &[("tmp", CType::int_signed())],
            &["rec"],
        )]);

        let result = analyze(&prog, Some(0x8000));

        assert!(result.is_stack_mode("rec"));
        assert_eq!(result.local_addr("rec", "n"), None);
        assert_eq!(result.local_addr("rec", "tmp"), None);
        assert_eq!(result.next_addr, 0x8000); // nothing allocated
    }

    #[test]
    fn mixed_recursive_and_non_recursive() {
        let prog = make_program(vec![
            make_func_with_vars(
                "main",
                &[],
                &[("retval", CType::int_signed())],
                &["helper", "rec"],
            ),
            make_func_with_vars(
                "helper",
                &[("a", CType::int_signed())],
                &[],
                &[],
            ),
            make_func_with_vars(
                "rec",
                &[("n", CType::int_signed())],
                &[("tmp", CType::int_signed())],
                &["rec"],
            ),
        ]);

        let result = analyze(&prog, Some(0x8000));

        // main is not recursive
        assert!(!result.is_stack_mode("main"));
        assert!(result.local_addr("main", "retval").is_some());

        // helper is not recursive
        assert!(!result.is_stack_mode("helper"));
        assert!(result.local_addr("helper", "a").is_some());

        // rec is recursive — no static allocation
        assert!(result.is_stack_mode("rec"));
        assert_eq!(result.local_addr("rec", "n"), None);
        assert_eq!(result.local_addr("rec", "tmp"), None);
    }

    #[test]
    fn sibling_functions_share_static_frame_space() {
        let prog = make_program(vec![
            make_func_with_vars("main", &[], &[("x", CType::int_signed())], &["left", "right"]),
            make_func_with_vars("left", &[], &[("tmp", CType::int_signed())], &[]),
            make_func_with_vars("right", &[], &[("tmp", CType::int_signed())], &[]),
        ]);

        let result = analyze(&prog, Some(0x8000));

        assert_ne!(result.local_addr("main", "x"), result.local_addr("left", "tmp"));
        assert_eq!(result.local_addr("left", "tmp"), result.local_addr("right", "tmp"));
        assert_eq!(result.next_addr, 0x8004);
    }

    #[test]
    fn descendant_function_does_not_share_frame_space() {
        let prog = make_program(vec![
            make_func_with_vars("main", &[], &[("x", CType::int_signed())], &["child"]),
            make_func_with_vars("child", &[], &[("tmp", CType::int_signed())], &[]),
        ]);

        let result = analyze(&prog, Some(0x8000));

        assert_ne!(result.local_addr("main", "x"), result.local_addr("child", "tmp"));
        assert_eq!(result.next_addr, 0x8004);
    }

    #[test]
    fn local_only_function_is_pure_and_readonly() {
        let prog = make_program(vec![make_func_with_vars(
            "helper",
            &[("a", CType::int_signed())],
            &[("tmp", CType::int_signed())],
            &[],
        )]);

        let result = analyze(&prog, Some(0x8000));
        let effects = result.effects_for("helper");

        assert!(effects.leaf);
        assert!(effects.pure);
        assert!(effects.readonly);
        assert!(!effects.noreturn);
    }

    #[test]
    fn runtime_helper_summary_is_available() {
        let prog = make_program(vec![]);
        let result = analyze(&prog, Some(0x8000));
        let effects = result.effects_for("__mul16");

        assert!(effects.leaf);
        assert!(effects.pure);
        assert!(effects.readonly);
        assert!(effects.clobbers.contains(&PhysReg::HL));
    }

    #[test]
    fn globals_before_locals() {
        let prog = IrProgram {
            globals: vec![GlobalVar {
                name: "g".into(),
                ty: CType::int_signed(),
                init: None,
            }],
            functions: vec![make_func_with_vars(
                "f",
                &[],
                &[("x", CType::int_signed())],
                &[],
            )],
            strings: vec![],
        };

        let result = analyze(&prog, Some(0x8000));

        // global g at 0x8000 (2 bytes)
        assert_eq!(result.global_addr("g"), Some(0x8000));
        // local x at 0x8002 (after globals)
        assert_eq!(result.local_addr("f", "x"), Some(0x8002));
        assert_eq!(result.next_addr, 0x8004);
    }

    // -- custom base address ---------------------------------------------

    #[test]
    fn custom_base_address() {
        let prog = make_program(vec![make_func_with_vars(
            "f",
            &[],
            &[("x", CType::int_signed())],
            &[],
        )]);

        let result = analyze(&prog, Some(0x4000));
        assert_eq!(result.local_addr("f", "x"), Some(0x4000));
    }

    #[test]
    fn default_base_address() {
        let prog = make_program(vec![make_func_with_vars(
            "f",
            &[],
            &[("x", CType::int_signed())],
            &[],
        )]);

        let result = analyze(&prog, None);
        assert_eq!(result.local_addr("f", "x"), Some(DEFAULT_BASE_ADDR));
    }

    // -- CallGraph queries -----------------------------------------------

    #[test]
    fn callees_of_query() {
        let prog = make_program(vec![
            make_func("main", &["foo", "bar"]),
            make_func("foo", &[]),
            make_func("bar", &[]),
        ]);
        let result = analyze(&prog, None);

        let cs = result.graph.callees_of("main").unwrap();
        assert!(cs.contains("foo"));
        assert!(cs.contains("bar"));
        assert_eq!(cs.len(), 2);

        assert!(result.graph.callees_of("foo").unwrap().is_empty());
        assert!(result.graph.callees_of("unknown").is_none());
    }

    // -- three-node cycle ------------------------------------------------

    #[test]
    fn three_node_cycle() {
        // a -> b -> c -> a
        let prog = make_program(vec![
            make_func("a", &["b"]),
            make_func("b", &["c"]),
            make_func("c", &["a"]),
        ]);
        let result = analyze(&prog, None);
        assert!(result.is_recursive("a"));
        assert!(result.is_recursive("b"));
        assert!(result.is_recursive("c"));
    }

    // -- long types -------------------------------------------------------

    #[test]
    fn long_variable_uses_four_bytes() {
        let prog = make_program(vec![make_func_with_vars(
            "f",
            &[],
            &[
                ("a", CType::long_signed()),
                ("b", CType::char_signed()),
            ],
            &[],
        )]);

        let result = analyze(&prog, Some(0x8000));
        assert_eq!(result.local_addr("f", "a"), Some(0x8000));
        // long = 4 bytes → b starts at 0x8004
        assert_eq!(result.local_addr("f", "b"), Some(0x8004));
        assert_eq!(result.next_addr, 0x8005);
    }

    // -- pointer variables ------------------------------------------------

    #[test]
    fn pointer_variable_uses_two_bytes() {
        let prog = make_program(vec![make_func_with_vars(
            "f",
            &[("p", CType::ptr(CType::char_signed()))],
            &[],
            &[],
        )]);

        let result = analyze(&prog, Some(0x8000));
        assert_eq!(result.local_addr("f", "p"), Some(0x8000));
        assert_eq!(result.next_addr, 0x8002);
    }
}
