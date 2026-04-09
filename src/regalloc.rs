//! Physical register allocator for the Intel 8080.
//!
//! The 8080 has an extremely constrained register set:
//!
//! | Register | Role |
//! |----------|------|
//! | A        | 8-bit accumulator |
//! | BC       | 16-bit pair (B=high, C=low) — loop counters, tertiary storage |
//! | DE       | 16-bit pair (D=high, E=low) — secondary 16-bit operand (DAD D) |
//! | HL       | 16-bit pair (H=high, L=low) — primary 16-bit accumulator, only pair for indirect addressing |
//! | SP       | Stack pointer — not general-purpose |
//!
//! Rather than a full graph-coloring allocator this module provides a
//! **demand-driven tracker** that the code generator queries:
//!
//! 1. "I need vreg X in HL" → the allocator moves/spills as needed.
//! 2. "Give me any free 16-bit register" → allocator picks one.
//! 3. "Free vreg X" → mark its register available.
//!
//! Spill slots are named memory locations (`__spill_0`, `__spill_1`, …).

use std::collections::HashMap;
use std::fmt;

use crate::ir::{VReg, Width};

// ---------------------------------------------------------------------------
// PhysReg — physical register names
// ---------------------------------------------------------------------------

/// Physical registers available for allocation on the 8080.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PhysReg {
    /// 8-bit accumulator.
    A,
    /// 16-bit register pair B (high) / C (low).
    BC,
    /// 16-bit register pair D (high) / E (low).
    DE,
    /// 16-bit register pair H (high) / L (low) — primary 16-bit accumulator.
    HL,
}

impl PhysReg {
    /// Natural width of this register.
    pub fn width(self) -> Width {
        match self {
            PhysReg::A => Width::W8,
            PhysReg::BC | PhysReg::DE | PhysReg::HL => Width::W16,
        }
    }

    /// Preference order for 16-bit allocation: HL first, then DE, then BC.
    pub const PAIR_PREF: [PhysReg; 3] = [PhysReg::HL, PhysReg::DE, PhysReg::BC];
}

impl fmt::Display for PhysReg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PhysReg::A => write!(f, "A"),
            PhysReg::BC => write!(f, "BC"),
            PhysReg::DE => write!(f, "DE"),
            PhysReg::HL => write!(f, "HL"),
        }
    }
}

// ---------------------------------------------------------------------------
// Location — where a virtual register currently lives
// ---------------------------------------------------------------------------

/// Current physical location of a virtual register's value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Location {
    /// Resident in a physical register.
    Reg(PhysReg),
    /// Spilled to a named memory location (label in the assembly output).
    Memory(String),
    /// Value can be rematerialized as an immediate, avoiding spill reloads.
    RematImm(i64),
    /// Value can be rematerialized as an address-of-global label.
    RematLabel(String),
}

// ---------------------------------------------------------------------------
// MoveOp — instructions the code generator must emit
// ---------------------------------------------------------------------------

/// A single data-movement operation the code generator must emit after an
/// allocation decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MoveOp {
    /// Store the contents of `src` to the named memory location.
    /// `width` is the natural width of the value (determined by `src.width()`).
    Spill { src: PhysReg, label: String, width: Width },
    /// Load from the named memory location into `dst`.
    /// `width` is the width of the stored value (may differ from `dst.width()`
    /// when a narrow value is zero-extended into a wider register pair).
    Reload { dst: PhysReg, label: String, width: Width },
    /// Materialize an immediate constant directly in `dst`.
    LoadImm { dst: PhysReg, value: i64, width: Width },
    /// Materialize an address-of-global label directly in `dst`.
    LoadLabel { dst: PhysReg, label: String },
    /// Register-to-register move.
    RegToReg { src: PhysReg, dst: PhysReg },
}

// ---------------------------------------------------------------------------
// RegAllocator
// ---------------------------------------------------------------------------

/// Demand-driven physical register tracker for the Intel 8080.
///
/// The code generator drives allocation by requesting specific placements.
/// Each mutating method returns a `Vec<MoveOp>` describing the moves /
/// spills / reloads the caller must emit *before* using the result.
pub struct RegAllocator {
    /// Virtual register → current location.
    vreg_map: HashMap<u32, Location>,
    /// Physical register → virtual register currently residing there
    /// (`None` means the register is free).
    reg_contents: HashMap<PhysReg, Option<u32>>,
    /// Monotonically increasing counter for spill-slot names.
    spill_counter: u32,
    /// Vregs that can be regenerated cheaply as immediates.
    remat_imm: HashMap<u32, i64>,
    /// Vregs that can be regenerated cheaply as address-of-global labels.
    remat_label: HashMap<u32, String>,
}

impl RegAllocator {
    /// Create a new allocator with all physical registers free.
    pub fn new() -> Self {
        let mut reg_contents = HashMap::new();
        reg_contents.insert(PhysReg::A, None);
        reg_contents.insert(PhysReg::BC, None);
        reg_contents.insert(PhysReg::DE, None);
        reg_contents.insert(PhysReg::HL, None);
        Self {
            vreg_map: HashMap::new(),
            reg_contents,
            spill_counter: 0,
            remat_imm: HashMap::new(),
            remat_label: HashMap::new(),
        }
    }

    // -- queries ----------------------------------------------------------

    /// Where is `vreg` currently located?  Returns `None` if the allocator
    /// has never seen this vreg.
    pub fn get_location(&self, vreg: VReg) -> Option<&Location> {
        self.vreg_map.get(&vreg.id)
    }

    /// Which virtual register (if any) currently occupies `reg`?
    pub fn occupant(&self, reg: PhysReg) -> Option<u32> {
        self.reg_contents.get(&reg).copied().flatten()
    }

    /// Is `reg` currently free?
    pub fn is_free(&self, reg: PhysReg) -> bool {
        self.occupant(reg).is_none()
    }

    // -- core operations --------------------------------------------------

    /// Is the given vreg rematerializable (immediate or label)?
    fn is_remat(&self, vreg_id: u32) -> bool {
        self.remat_imm.contains_key(&vreg_id) || self.remat_label.contains_key(&vreg_id)
    }

    /// Generate a fresh spill-slot label.
    fn fresh_spill_label(&mut self) -> String {
        let label = format!("__spill_{}", self.spill_counter);
        self.spill_counter += 1;
        label
    }

    /// Allocate a fresh spill label without spilling any register.
    /// The caller is responsible for emitting the store instructions.
    pub fn alloc_spill_label(&mut self) -> String {
        self.fresh_spill_label()
    }

    /// Mark a vreg as living at the given memory label, without emitting
    /// any move instructions.  The caller must have already stored the
    /// value at `label`.
    pub fn mark_in_memory(&mut self, vreg: VReg, label: String) {
        self.vreg_map.insert(vreg.id, Location::Memory(label));
    }

    /// Spill the current occupant of `reg` to memory and return the
    /// [`MoveOp`] the caller must emit.  If the register is already free,
    /// returns `None`.
    pub fn spill(&mut self, reg: PhysReg) -> Option<MoveOp> {
        if let Some(vreg_id) = self.occupant(reg) {
            if let Some(&value) = self.remat_imm.get(&vreg_id) {
                self.vreg_map.insert(vreg_id, Location::RematImm(value));
                self.reg_contents.insert(reg, None);
                return None;
            }
            if let Some(lbl) = self.remat_label.get(&vreg_id).cloned() {
                self.vreg_map.insert(vreg_id, Location::RematLabel(lbl));
                self.reg_contents.insert(reg, None);
                return None;
            }
            let label = self.fresh_spill_label();
            self.vreg_map.insert(vreg_id, Location::Memory(label.clone()));
            self.reg_contents.insert(reg, None);
            Some(MoveOp::Spill { src: reg, label, width: reg.width() })
        } else {
            None
        }
    }

    /// Spill the occupant of `reg` to a *specific* label instead of
    /// generating a fresh spill slot.  Useful when the code generator knows
    /// the variable's home location (e.g. a global or stack slot name).
    pub fn spill_to_label(&mut self, reg: PhysReg, label: &str) -> Option<MoveOp> {
        if let Some(vreg_id) = self.occupant(reg) {
            self.remat_imm.remove(&vreg_id);
            self.vreg_map
                .insert(vreg_id, Location::Memory(label.to_string()));
            self.reg_contents.insert(reg, None);
            Some(MoveOp::Spill {
                src: reg,
                label: label.to_string(),
                width: reg.width(),
            })
        } else {
            None
        }
    }

    /// Allocate any free register appropriate for `vreg`'s width, using the
    /// standard preference order.  Returns the chosen register and any
    /// moves that must be emitted (spills when nothing is free).
    pub fn allocate(&mut self, vreg: VReg) -> (PhysReg, Vec<MoveOp>) {
        // If already allocated in a register, just return it.
        if let Some(Location::Reg(r)) = self.vreg_map.get(&vreg.id) {
            return (*r, Vec::new());
        }

        let mut ops = Vec::new();
        let candidates: &[PhysReg] = match vreg.width {
            Width::W8 => &[PhysReg::A],
            Width::W16 | Width::W32 => &PhysReg::PAIR_PREF,
        };

        // Try to find a free register in preference order.
        for &reg in candidates {
            if self.is_free(reg) {
                return self.place_vreg(vreg, reg, ops);
            }
        }

        // Nothing free — spill the *last* in preference order.
        let victim = *candidates.last().unwrap();
        if let Some(spill_op) = self.spill(victim) {
            ops.push(spill_op);
        }
        self.place_vreg(vreg, victim, ops)
    }

    /// Ensure `vreg` is in the specific physical register `target`.
    /// Returns the list of moves the caller must emit.
    pub fn ensure_in_reg(&mut self, vreg: VReg, target: PhysReg) -> Vec<MoveOp> {
        let mut ops = Vec::new();

        // Already there?
        if let Some(Location::Reg(current)) = self.vreg_map.get(&vreg.id).cloned() {
            if current == target {
                return ops;
            }
            // Evict whoever is in target.
            if let Some(displaced_id) = self.occupant(target) {
                // Remat values can be re-generated cheaply; just drop them
                // instead of moving to another register.
                if self.is_remat(displaced_id) {
                    self.spill(target);
                } else if let Some(alt) = self.find_free_alternate(target) {
                    self.reg_contents.insert(target, None);
                    self.reg_contents.insert(alt, Some(displaced_id));
                    self.vreg_map.insert(displaced_id, Location::Reg(alt));
                    ops.push(MoveOp::RegToReg {
                        src: target,
                        dst: alt,
                    });
                } else if let Some(spill_op) = self.spill(target) {
                    ops.push(spill_op);
                }
            }
            // Move vreg from current → target.
            self.reg_contents.insert(current, None);
            self.reg_contents.insert(target, Some(vreg.id));
            self.vreg_map.insert(vreg.id, Location::Reg(target));
            ops.push(MoveOp::RegToReg {
                src: current,
                dst: target,
            });
            return ops;
        }

        // Vreg is in memory — evict target, then reload.
        if let Some(displaced_id) = self.occupant(target) {
            if self.is_remat(displaced_id) {
                self.spill(target);
            } else if let Some(alt) = self.find_free_alternate(target) {
                self.reg_contents.insert(target, None);
                self.reg_contents.insert(alt, Some(displaced_id));
                self.vreg_map.insert(displaced_id, Location::Reg(alt));
                ops.push(MoveOp::RegToReg {
                    src: target,
                    dst: alt,
                });
            } else if let Some(spill_op) = self.spill(target) {
                ops.push(spill_op);
            }
        }

        if let Some(Location::Memory(label)) = self.vreg_map.get(&vreg.id).cloned() {
            // Reloading a W8 value into a pair register uses "LXI H,label; MOV r,M"
            // which temporarily clobbers HL. Pre-spill HL now so the allocator
            // state stays consistent with what emit_moves will actually emit.
            if vreg.width == Width::W8 && matches!(target, PhysReg::BC | PhysReg::DE) {
                if let Some(spill_op) = self.spill(PhysReg::HL) {
                    ops.push(spill_op);
                }
            }
            ops.push(MoveOp::Reload {
                dst: target,
                label,
                width: vreg.width,
            });
        } else if let Some(Location::RematImm(value)) = self.vreg_map.get(&vreg.id).cloned() {
            ops.push(MoveOp::LoadImm {
                dst: target,
                value,
                width: vreg.width,
            });
        } else if let Some(Location::RematLabel(lbl)) = self.vreg_map.get(&vreg.id).cloned() {
            ops.push(MoveOp::LoadLabel {
                dst: target,
                label: lbl,
            });
        }
        // (If vreg is completely new, the code gen will emit a load itself;
        // we just mark the register occupied.)

        self.reg_contents.insert(target, Some(vreg.id));
        self.vreg_map.insert(vreg.id, Location::Reg(target));
        ops
    }

    /// Free the physical register currently holding `vreg`.  After this call
    /// the vreg has *no* location — use only when the value is dead.
    pub fn free(&mut self, vreg: VReg) {
        if let Some(Location::Reg(reg)) = self.vreg_map.remove(&vreg.id) {
            self.reg_contents.insert(reg, None);
        } else {
            self.vreg_map.remove(&vreg.id);
        }
        self.remat_imm.remove(&vreg.id);
        self.remat_label.remove(&vreg.id);
    }

    /// Mark a physical register as directly occupied by `vreg` (e.g. after
    /// the code generator has emitted a load itself).
    pub fn mark_allocated(&mut self, vreg: VReg, reg: PhysReg) -> Vec<MoveOp> {
        let mut ops = Vec::new();
        // Evict current occupant if different: prefer a free alternate register
        // over a memory spill (e.g. move HL → DE rather than SHLD __spill_N).
        if let Some(old_id) = self.occupant(reg) {
            if old_id != vreg.id {
                if self.is_remat(old_id) {
                    self.spill(reg);
                } else if let Some(alt) = self.find_free_alternate(reg) {
                    self.reg_contents.insert(reg, None);
                    self.reg_contents.insert(alt, Some(old_id));
                    self.vreg_map.insert(old_id, Location::Reg(alt));
                    ops.push(MoveOp::RegToReg { src: reg, dst: alt });
                } else if let Some(op) = self.spill(reg) {
                    ops.push(op);
                }
            }
        }
        self.reg_contents.insert(reg, Some(vreg.id));
        self.vreg_map.insert(vreg.id, Location::Reg(reg));
        self.remat_imm.remove(&vreg.id);
        self.remat_label.remove(&vreg.id);
        ops
    }

    /// Mark a register allocation that is rematerializable as an immediate.
    pub fn mark_immediate(&mut self, vreg: VReg, reg: PhysReg, value: i64) -> Vec<MoveOp> {
        let ops = self.mark_allocated(vreg, reg);
        self.remat_imm.insert(vreg.id, value);
        ops
    }

    /// Record `vreg` as a rematerializable immediate WITHOUT allocating a
    /// physical register.  Use this when the value is small enough that the
    /// INX/DCX fast path will consume it directly from `known_imm()` rather
    /// than from a register, so no LXI instruction needs to be emitted eagerly.
    /// `ensure_de`/`ensure_hl` will emit the load lazily if the register is
    /// ever actually needed.
    pub fn mark_remat_imm_only(&mut self, vreg: VReg, value: i64) {
        self.vreg_map.insert(vreg.id, Location::RematImm(value));
        self.remat_imm.insert(vreg.id, value);
    }

    /// Return the tracked immediate value for a vreg when rematerialization
    /// is available.
    pub fn immediate_of(&self, vreg: VReg) -> Option<i64> {
        self.remat_imm.get(&vreg.id).copied()
    }

    /// Record `vreg` as a rematerializable address-of-global label WITHOUT
    /// allocating a physical register.  The load is deferred until the vreg
    /// is actually needed in a register, allowing `ensure_de` to emit
    /// `LXI D,label` directly instead of `LXI H,label; XCHG`.
    pub fn mark_remat_label_only(&mut self, vreg: VReg, label: String) {
        self.vreg_map.insert(vreg.id, Location::RematLabel(label.clone()));
        self.remat_label.insert(vreg.id, label);
    }

    /// Return the tracked label for a vreg when rematerialization is available.
    pub fn label_of(&self, vreg: VReg) -> Option<&str> {
        self.remat_label.get(&vreg.id).map(|s| s.as_str())
    }

    /// Save all live registers to memory (e.g. before a CALL).
    /// Returns the list of spill operations.
    pub fn save_all(&mut self) -> Vec<MoveOp> {
        let mut ops = Vec::new();
        for &reg in &[PhysReg::HL, PhysReg::DE, PhysReg::BC, PhysReg::A] {
            if let Some(op) = self.spill(reg) {
                ops.push(op);
            }
        }
        ops
    }

    /// Reset the allocator — all registers free, all vreg mappings cleared.
    pub fn reset(&mut self) {
        self.vreg_map.clear();
        self.remat_imm.clear();
        self.remat_label.clear();
        for val in self.reg_contents.values_mut() {
            *val = None;
        }
    }

    /// Forget any values currently believed to live in `reg`.
    ///
    /// This is used after instructions such as `CALL` that clobber machine
    /// registers without preserving the previous contents.
    pub fn clobber(&mut self, reg: PhysReg) {
        if let Some(vreg_id) = self.occupant(reg) {
            self.vreg_map.remove(&vreg_id);
        }
        self.reg_contents.insert(reg, None);
    }

    /// Forget any values currently believed to live in the given registers.
    pub fn clobber_regs(&mut self, regs: &[PhysReg]) {
        for &reg in regs {
            self.clobber(reg);
        }
    }

    // -- helpers ----------------------------------------------------------

    /// Place `vreg` into `reg`, appending a reload from memory if the vreg
    /// was previously spilled.
    fn place_vreg(
        &mut self,
        vreg: VReg,
        reg: PhysReg,
        mut ops: Vec<MoveOp>,
    ) -> (PhysReg, Vec<MoveOp>) {
        // If vreg was in memory, emit a reload.
        if let Some(Location::Memory(label)) = self.vreg_map.get(&vreg.id) {
            ops.push(MoveOp::Reload {
                dst: reg,
                label: label.clone(),
                width: vreg.width,
            });
        } else if let Some(Location::RematImm(value)) = self.vreg_map.get(&vreg.id) {
            ops.push(MoveOp::LoadImm {
                dst: reg,
                value: *value,
                width: vreg.width,
            });
        } else if let Some(Location::RematLabel(lbl)) = self.vreg_map.get(&vreg.id) {
            ops.push(MoveOp::LoadLabel {
                dst: reg,
                label: lbl.clone(),
            });
        }
        self.reg_contents.insert(reg, Some(vreg.id));
        self.vreg_map.insert(vreg.id, Location::Reg(reg));
        (reg, ops)
    }

    fn find_free_alternate(&self, target: PhysReg) -> Option<PhysReg> {
        match target.width() {
            Width::W8 => None,
            Width::W16 => PhysReg::PAIR_PREF
                .iter()
                .copied()
                .find(|r| *r != target && self.is_free(*r)),
            Width::W32 => None,
        }
    }
}

impl Default for RegAllocator {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{VReg, Width};

    fn v16(id: u32) -> VReg {
        VReg::new(id, Width::W16)
    }

    fn v8(id: u32) -> VReg {
        VReg::new(id, Width::W8)
    }

    #[test]
    fn allocate_16bit_prefers_hl() {
        let mut ra = RegAllocator::new();
        let (reg, ops) = ra.allocate(v16(0));
        assert_eq!(reg, PhysReg::HL);
        assert!(ops.is_empty());
        assert_eq!(ra.get_location(v16(0)), Some(&Location::Reg(PhysReg::HL)));
    }

    #[test]
    fn allocate_8bit_prefers_a() {
        let mut ra = RegAllocator::new();
        let (reg, ops) = ra.allocate(v8(0));
        assert_eq!(reg, PhysReg::A);
        assert!(ops.is_empty());
    }

    #[test]
    fn allocate_second_pair_gets_de() {
        let mut ra = RegAllocator::new();
        ra.allocate(v16(0)); // gets HL
        let (reg, ops) = ra.allocate(v16(1));
        assert_eq!(reg, PhysReg::DE);
        assert!(ops.is_empty());
    }

    #[test]
    fn allocate_third_pair_gets_bc() {
        let mut ra = RegAllocator::new();
        ra.allocate(v16(0)); // HL
        ra.allocate(v16(1)); // DE
        let (reg, ops) = ra.allocate(v16(2));
        assert_eq!(reg, PhysReg::BC);
        assert!(ops.is_empty());
    }

    #[test]
    fn allocate_fourth_pair_spills() {
        let mut ra = RegAllocator::new();
        ra.allocate(v16(0)); // HL
        ra.allocate(v16(1)); // DE
        ra.allocate(v16(2)); // BC
        let (reg, ops) = ra.allocate(v16(3));
        // Should spill BC (last in preference) and reuse it.
        assert_eq!(reg, PhysReg::BC);
        assert_eq!(ops.len(), 1);
        match &ops[0] {
            MoveOp::Spill { src, label, .. } => {
                assert_eq!(*src, PhysReg::BC);
                assert!(label.starts_with("__spill_"));
            }
            other => panic!("expected Spill, got {:?}", other),
        }
    }

    #[test]
    fn ensure_in_reg_moves_between_regs() {
        let mut ra = RegAllocator::new();
        ra.allocate(v16(0)); // HL
        let ops = ra.ensure_in_reg(v16(0), PhysReg::DE);
        assert_eq!(ops.len(), 1);
        assert_eq!(
            ops[0],
            MoveOp::RegToReg {
                src: PhysReg::HL,
                dst: PhysReg::DE,
            }
        );
        assert_eq!(
            ra.get_location(v16(0)),
            Some(&Location::Reg(PhysReg::DE))
        );
        assert!(ra.is_free(PhysReg::HL));
    }

    #[test]
    fn ensure_in_reg_evicts_occupant() {
        let mut ra = RegAllocator::new();
        ra.allocate(v16(0)); // HL
        ra.allocate(v16(1)); // DE
        // Move vreg 1 to HL — relocate vreg 0 to BC first.
        let ops = ra.ensure_in_reg(v16(1), PhysReg::HL);
        assert_eq!(ops.len(), 2);
        assert_eq!(
            ops[0],
            MoveOp::RegToReg {
                src: PhysReg::HL,
                dst: PhysReg::BC,
            }
        );
        // Second op: move vreg 1 from DE to HL.
        assert_eq!(
            ops[1],
            MoveOp::RegToReg {
                src: PhysReg::DE,
                dst: PhysReg::HL,
            }
        );
        assert_eq!(
            ra.get_location(v16(1)),
            Some(&Location::Reg(PhysReg::HL))
        );
    }

    #[test]
    fn ensure_in_reg_reloads_from_memory() {
        let mut ra = RegAllocator::new();
        ra.allocate(v16(0)); // HL
        ra.allocate(v16(1)); // DE
        ra.allocate(v16(2)); // BC
        // Allocating a fourth spills BC (vreg 2) and takes BC for vreg 3.
        ra.allocate(v16(3));
        // Now vreg 2 is in memory.  Ask for it in DE.
        let ops = ra.ensure_in_reg(v16(2), PhysReg::DE);
        // Should spill DE (vreg 1), then reload vreg 2 into DE.
        assert!(ops.len() >= 2);
        match &ops[0] {
            MoveOp::Spill { src, .. } => assert_eq!(*src, PhysReg::DE),
            other => panic!("expected Spill, got {:?}", other),
        }
        match &ops[1] {
            MoveOp::Reload { dst, label, .. } => {
                assert_eq!(*dst, PhysReg::DE);
                assert!(label.starts_with("__spill_"));
            }
            other => panic!("expected Reload, got {:?}", other),
        }
    }

    #[test]
    fn free_releases_register() {
        let mut ra = RegAllocator::new();
        ra.allocate(v16(0)); // HL
        assert!(!ra.is_free(PhysReg::HL));
        ra.free(v16(0));
        assert!(ra.is_free(PhysReg::HL));
        assert!(ra.get_location(v16(0)).is_none());
    }

    #[test]
    fn save_all_spills_live_registers() {
        let mut ra = RegAllocator::new();
        ra.allocate(v16(0)); // HL
        ra.allocate(v16(1)); // DE
        let ops = ra.save_all();
        assert_eq!(ops.len(), 2);
        // After save_all, everything is free.
        assert!(ra.is_free(PhysReg::HL));
        assert!(ra.is_free(PhysReg::DE));
        assert!(ra.is_free(PhysReg::BC));
        assert!(ra.is_free(PhysReg::A));
    }

    #[test]
    fn mark_allocated_evicts_previous_occupant() {
        let mut ra = RegAllocator::new();
        ra.allocate(v16(0)); // HL
        let ops = ra.mark_allocated(v16(1), PhysReg::HL);
        assert_eq!(ops.len(), 1);
        // With a free alternate register (DE or BC) available, mark_allocated
        // should prefer a register-to-register move over a memory spill.
        match &ops[0] {
            MoveOp::RegToReg { src, .. } => assert_eq!(*src, PhysReg::HL),
            other => panic!("expected RegToReg, got {:?}", other),
        }
        assert_eq!(ra.occupant(PhysReg::HL), Some(1));
    }

    #[test]
    fn mark_allocated_spills_when_no_free_alternate() {
        let mut ra = RegAllocator::new();
        // Fill all 16-bit regs so there's no free alternate.
        ra.allocate(v16(0)); // HL
        ra.allocate(v16(1)); // DE
        ra.allocate(v16(2)); // BC
        let ops = ra.mark_allocated(v16(3), PhysReg::HL);
        assert_eq!(ops.len(), 1);
        match &ops[0] {
            MoveOp::Spill { src, .. } => assert_eq!(*src, PhysReg::HL),
            other => panic!("expected Spill, got {:?}", other),
        }
        assert_eq!(ra.occupant(PhysReg::HL), Some(3));
    }

    #[test]
    fn spill_to_label_uses_given_name() {
        let mut ra = RegAllocator::new();
        ra.allocate(v16(0)); // HL
        let op = ra.spill_to_label(PhysReg::HL, "_my_var");
        assert_eq!(
            op,
            Some(MoveOp::Spill {
                src: PhysReg::HL,
                label: "_my_var".to_string(),
                width: Width::W16,
            })
        );
        assert_eq!(
            ra.get_location(v16(0)),
            Some(&Location::Memory("_my_var".to_string()))
        );
        assert!(ra.is_free(PhysReg::HL));
    }

    #[test]
    fn allocate_already_in_register_is_noop() {
        let mut ra = RegAllocator::new();
        let (reg1, _) = ra.allocate(v16(0));
        let (reg2, ops2) = ra.allocate(v16(0));
        assert_eq!(reg1, reg2);
        assert!(ops2.is_empty());
    }

    #[test]
    fn reset_clears_everything() {
        let mut ra = RegAllocator::new();
        ra.allocate(v16(0));
        ra.allocate(v16(1));
        ra.reset();
        assert!(ra.is_free(PhysReg::HL));
        assert!(ra.is_free(PhysReg::DE));
        assert!(ra.is_free(PhysReg::BC));
        assert!(ra.is_free(PhysReg::A));
        assert!(ra.get_location(v16(0)).is_none());
    }

    #[test]
    fn phys_reg_display() {
        assert_eq!(PhysReg::A.to_string(), "A");
        assert_eq!(PhysReg::BC.to_string(), "BC");
        assert_eq!(PhysReg::DE.to_string(), "DE");
        assert_eq!(PhysReg::HL.to_string(), "HL");
    }

    #[test]
    fn phys_reg_width() {
        assert_eq!(PhysReg::A.width(), Width::W8);
        assert_eq!(PhysReg::BC.width(), Width::W16);
        assert_eq!(PhysReg::DE.width(), Width::W16);
        assert_eq!(PhysReg::HL.width(), Width::W16);
    }
}
