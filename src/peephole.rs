//! Peephole optimizer for Intel 8080 assembly.
//!
//! Performs pattern-matched rewriting on the assembly text produced by
//! [`crate::codegen`].  The optimizer repeatedly scans the instruction
//! stream, applying local rewrite rules (windows of 2–3 instructions)
//! until a fixed-point is reached (no rule fires).
//!
//! ## Assembly format assumptions
//!
//! | Kind        | Format                     |
//! |-------------|----------------------------|
//! | Instruction | `\tOPCODE operands`        |
//! | Label       | `name:` (no leading tab)   |
//! | Comment     | `; text`                   |
//! | Empty       | blank line                 |

// ---------------------------------------------------------------------------
// Parsed line representation
// ---------------------------------------------------------------------------

/// A single assembly line in a structured form.
#[derive(Debug, Clone, PartialEq)]
enum Line {
    /// A label definition (e.g. `func_name:`).  Stored *without* the colon.
    Label(String),
    /// An instruction with an opcode and optional operands.
    Instruction { opcode: String, operands: String },
    /// A comment line (starts with `;`).
    Comment(String),
    /// A blank / whitespace-only line.
    Empty,
}

/// Parse a raw assembly string into a [`Line`].
fn parse_line(raw: &str) -> Line {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Line::Empty;
    }
    if trimmed.starts_with(';') {
        return Line::Comment(raw.to_string());
    }
    // Labels have no leading whitespace and end with ':'
    if !raw.starts_with('\t') && !raw.starts_with(' ') && trimmed.ends_with(':') {
        let name = trimmed.trim_end_matches(':').to_string();
        return Line::Label(name);
    }
    // Instructions: split on first whitespace after the opcode.
    let inst = trimmed;
    if let Some(pos) = inst.find(|c: char| c == ' ' || c == '\t') {
        let opcode = inst[..pos].to_uppercase();
        let operands = inst[pos..].trim().to_string();
        Line::Instruction { opcode, operands }
    } else {
        Line::Instruction {
            opcode: inst.to_uppercase(),
            operands: String::new(),
        }
    }
}

/// Render a [`Line`] back to its assembly text representation.
fn render_line(line: &Line) -> String {
    match line {
        Line::Label(name) => format!("{}:", name),
        Line::Instruction { opcode, operands } => {
            if operands.is_empty() {
                format!("\t{}", opcode)
            } else {
                format!("\t{} {}", opcode, operands)
            }
        }
        Line::Comment(text) => text.clone(),
        Line::Empty => String::new(),
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Return `true` if the opcode is an unconditional jump.
fn is_unconditional_jump(opcode: &str) -> bool {
    opcode == "JMP"
}

/// Return `true` if the opcode is a conditional jump.
fn is_conditional_jump(opcode: &str) -> bool {
    matches!(opcode, "JZ" | "JNZ" | "JC" | "JNC" | "JM" | "JP" | "JPE" | "JPO")
}

/// Return `true` if the line is a label definition.
fn is_label(line: &Line) -> bool {
    matches!(line, Line::Label(_))
}

/// Return `true` if the instruction is a `MOV X,X` (self-move).
fn is_self_move(opcode: &str, operands: &str) -> bool {
    if opcode != "MOV" {
        return false;
    }
    let parts: Vec<&str> = operands.split(',').map(str::trim).collect();
    parts.len() == 2 && parts[0] == parts[1]
}

// ---------------------------------------------------------------------------
// Peephole rules
// ---------------------------------------------------------------------------

/// Apply all peephole rules to `lines`.  Returns `true` if any change was
/// made (so the caller knows to iterate again).
fn apply_rules(lines: &mut Vec<Line>) -> bool {
    let mut changed = false;

    // --- Rule 15: Remove NOP instructions --------------------------------
    changed |= rule_remove_nop(lines);

    // --- Rule 4: Remove self-move (MOV X,X) ------------------------------
    changed |= rule_remove_self_move(lines);

    // --- Rule 16: Remove double XCHG (XCHG / XCHG → nothing) ------------
    changed |= rule_remove_double_xchg(lines);

    // --- Rule 24: Remove double CMA (CMA / CMA → nothing) ---------------
    changed |= rule_remove_double_cma(lines);

    // --- Rule 13: Merge adjacent labels ----------------------------------
    changed |= rule_merge_adjacent_labels(lines);

    // --- Rule 35: Inline main into CRT0 startup --------------------------
    changed |= rule_inline_main(lines);

    // --- Two-instruction window rules ------------------------------------
    changed |= rule_two_window(lines);

    // --- Rule 19: Conditional branch inversion ---------------------------
    changed |= rule_branch_inversion(lines);

    // --- Rule 18: Remove jump to next label ------------------------------
    changed |= rule_jump_to_next(lines);

    // --- Rule 33: Remove conditional jump to next label ------------------
    changed |= rule_conditional_jump_to_next(lines);

    // --- Rule 34: Jump to RET folding ------------------------------------
    changed |= rule_jump_to_ret(lines);

    // --- Rule 10: Remove dead code after unconditional jump --------------
    changed |= rule_dead_code_after_jump(lines);

    // --- Rule 21: MVI A,0 → XRA A (smaller) -----------------------------
    changed |= rule_mvi_a_zero_to_xra(lines);

    // --- Rule 25/26: Remove INX/DCX pairs that cancel --------------------
    changed |= rule_remove_inx_dcx_pairs(lines);

    // --- Rule 28/29: Remove redundant LDA/STA pairs ----------------------
    changed |= rule_lda_sta_pairs(lines);

    // --- Rule 37 & 38: Redundant LXI H,N / LHLD when HL already holds value ---
    changed |= rule_elim_redundant_lxi_h(lines);

    // --- Rule 43: LXI H,N / MOV E,M / INX H / MOV D,M / XCHG → LHLD N ---
    changed |= rule_lxi_w16_load_to_lhld(lines);

    // --- Rule 40: Remove dead spill stores (__spill_ labels never read) --
    changed |= rule_elim_dead_spills(lines);

    // --- Rule 41: Remove unused _l_ / __spill_ .STORAGE declarations ----
    changed |= rule_elim_unused_storage(lines);

    // --- Rule 32: Jump threading (resolve JMP chains) --------------------
    changed |= rule_jump_threading(lines);

    // --- Rule 31: Remove unreferenced labels (except function labels) ----
    changed |= rule_remove_unreferenced_labels(lines);

    changed
}

// ---------------------------------------------------------------------------
// Individual rules
// ---------------------------------------------------------------------------

/// Rule 15 – Remove `NOP` instructions.
fn rule_remove_nop(lines: &mut Vec<Line>) -> bool {
    let before = lines.len();
    lines.retain(|l| {
        !matches!(l, Line::Instruction { opcode, .. } if opcode == "NOP")
    });
    lines.len() != before
}

/// Rule 4 – Remove self-moves (`MOV A,A`, `MOV H,H`, etc.).
fn rule_remove_self_move(lines: &mut Vec<Line>) -> bool {
    let before = lines.len();
    lines.retain(|l| {
        !matches!(l, Line::Instruction { opcode, operands }
            if is_self_move(opcode, operands))
    });
    lines.len() != before
}

/// Rule 13 – Merge adjacent labels.
///
/// When `L1:` is immediately followed by `L2:`, rewrite every reference to
/// `L2` so it points to `L1`, then delete `L2:`.
fn rule_merge_adjacent_labels(lines: &mut Vec<Line>) -> bool {
    // Build rename map: later label → earlier label.
    let mut rename: std::collections::HashMap<String, String> = std::collections::HashMap::new();

    let mut i = 0;
    while i + 1 < lines.len() {
        if let (Line::Label(a), Line::Label(b)) = (&lines[i], &lines[i + 1]) {
            // b is the later label; rename it to a (follow transitive renames).
            let target = rename.get(a).cloned().unwrap_or_else(|| a.clone());
            rename.insert(b.clone(), target);
            i += 2;
        } else {
            i += 1;
        }
    }

    if rename.is_empty() {
        return false;
    }

    // Rewrite operand references and delete merged labels.
    let mut out: Vec<Line> = Vec::with_capacity(lines.len());
    for line in lines.iter() {
        match line {
            Line::Label(name) if rename.contains_key(name) => {
                // Drop the merged label.
            }
            Line::Instruction { opcode, operands } => {
                let new_operands = rename
                    .get(operands.trim())
                    .cloned()
                    .unwrap_or_else(|| operands.clone());
                out.push(Line::Instruction {
                    opcode: opcode.clone(),
                    operands: new_operands,
                });
            }
            other => out.push(other.clone()),
        }
    }

    *lines = out;
    true
}

/// Rules 1, 2, 3, 5, 7, 8, 12 – Two/three instruction window rules.
///
/// Scans a sliding window of consecutive *instructions* (skipping labels,
/// comments, and blanks) and applies the first matching rewrite.
fn rule_two_window(lines: &mut Vec<Line>) -> bool {
    let mut changed = false;
    let mut i = 0;
    while i + 1 < lines.len() {
        // Find the next pair of instructions.
        let a = &lines[i];
        let b = &lines[i + 1];

        match (a, b) {
            // --- Rule 1: SHLD addr / LHLD addr → SHLD addr --------------
            (
                Line::Instruction { opcode: op_a, operands: addr_a },
                Line::Instruction { opcode: op_b, operands: addr_b },
            ) if op_a == "SHLD" && op_b == "LHLD" && addr_a == addr_b => {
                lines.remove(i + 1);
                changed = true;
            }

            // --- Rule 2: LHLD addr / SHLD addr → LHLD addr --------------
            (
                Line::Instruction { opcode: op_a, operands: addr_a },
                Line::Instruction { opcode: op_b, operands: addr_b },
            ) if op_a == "LHLD" && op_b == "SHLD" && addr_a == addr_b => {
                lines.remove(i + 1);
                changed = true;
            }

            // --- Rule 3: CALL func / RET → JMP func ---------------------
            (
                Line::Instruction { opcode: op_a, operands: func },
                Line::Instruction { opcode: op_b, .. },
            ) if op_a == "CALL" && op_b == "RET" => {
                lines[i] = Line::Instruction {
                    opcode: "JMP".to_string(),
                    operands: func.clone(),
                };
                lines.remove(i + 1);
                changed = true;
            }

            // --- Rule 7: LHLD addr / LHLD addr → single LHLD addr ------
            (
                Line::Instruction { opcode: op_a, operands: addr_a },
                Line::Instruction { opcode: op_b, operands: addr_b },
            ) if op_a == "LHLD" && op_b == "LHLD" && addr_a == addr_b => {
                lines.remove(i + 1);
                changed = true;
            }

            // --- Rule 8: SHLD addr / SHLD addr → single SHLD addr ------
            (
                Line::Instruction { opcode: op_a, operands: addr_a },
                Line::Instruction { opcode: op_b, operands: addr_b },
            ) if op_a == "SHLD" && op_b == "SHLD" && addr_a == addr_b => {
                lines.remove(i + 1);
                changed = true;
            }

            // --- Rule 12: PUSH X / POP X → deleted ----------------------
            (
                Line::Instruction { opcode: op_a, operands: reg_a },
                Line::Instruction { opcode: op_b, operands: reg_b },
            ) if op_a == "PUSH" && op_b == "POP" && reg_a.trim() == reg_b.trim() => {
                lines.remove(i + 1);
                lines.remove(i);
                changed = true;
                // Don't advance i; re-examine at the current position.
                continue;
            }

            // --- Rule 39: LXI H,N / LHLD X → LHLD X  (LXI dead) ----------
            // HL is immediately overwritten by LHLD, so the preceding LXI H,N
            // is dead and can be removed.  This cleans up the residue left by
            // the INX H / DCX H fast path in gen_add/gen_sub when the constant
            // vreg was placed in HL by gen_load_imm.
            (
                Line::Instruction { opcode: op_a, operands: ops_a },
                Line::Instruction { opcode: op_b, .. },
            ) if op_a == "LXI" && op_b == "LHLD"
                && ops_a.trim().starts_with("H,") =>
            {
                lines.remove(i);
                changed = true;
                continue;
            }

            // --- Rule 42: LXI r,N1 / LXI r,N2 → LXI r,N2  (dead LXI) ---
            // When gen_load_imm places a constant in DE (or BC) to avoid
            // spilling HL, and the very next instruction overwrites the same
            // register (e.g. gen_sub re-loads DE with the negated value), the
            // first LXI is dead and can be removed.
            (
                Line::Instruction { opcode: op_a, operands: ops_a },
                Line::Instruction { opcode: op_b, operands: ops_b },
            ) if op_a == "LXI" && op_b == "LXI" => {
                let r_a = ops_a.trim().split(',').next().unwrap_or("");
                let r_b = ops_b.trim().split(',').next().unwrap_or("");
                if !r_a.is_empty() && r_a == r_b && r_a != "H" {
                    // First LXI to the same non-HL pair is dead.
                    lines.remove(i);
                    changed = true;
                    continue;
                }
            }

            // --- Rule 36: MVI A,n / MOV M,A → MVI M,n ------------------
            // MVI M,imm8 stores the immediate directly to (HL) in one
            // instruction (2 bytes) instead of two (MVI A,n = 2 bytes,
            // MOV M,A = 1 byte, total 3 bytes).
            (
                Line::Instruction { opcode: op_a, operands: imm },
                Line::Instruction { opcode: op_b, operands: ops_b },
            ) if op_a == "MVI" && op_b == "MOV"
                && imm.trim().starts_with("A,")
                && ops_b.trim() == "M,A" =>
            {
                let immediate = imm.trim()["A,".len()..].to_string();
                lines[i] = Line::Instruction {
                    opcode: "MVI".to_string(),
                    operands: format!("M,{}", immediate),
                };
                lines.remove(i + 1);
                changed = true;
                continue;
            }

            _ => {}
        }

        // --- Rule 5: ORA L / CPI 0 → just ORA L  (three-instr window) --
        // ORA already sets the zero flag, so a subsequent CPI 0 is dead.
        if i + 1 < lines.len() {
            if let (
                Line::Instruction { opcode: op_a, operands: _ },
                Line::Instruction { opcode: op_b, operands: imm },
            ) = (&lines[i], &lines[i + 1])
            {
                if op_a == "ORA" && op_b == "CPI" && imm.trim() == "0" {
                    lines.remove(i + 1);
                    changed = true;
                }
            }
        }

        i += 1;
    }
    changed
}

/// Rule 10 – Remove dead code after an unconditional jump.
///
/// Any non-label instruction between a `JMP` and the next label is
/// unreachable and can be deleted.
fn rule_dead_code_after_jump(lines: &mut Vec<Line>) -> bool {
    let mut changed = false;
    let mut i = 0;
    while i < lines.len() {
        if let Line::Instruction { opcode, .. } = &lines[i] {
            if is_unconditional_jump(opcode) {
                // Delete everything after this JMP until we hit a label.
                let mut j = i + 1;
                while j < lines.len() && !is_label(&lines[j]) {
                    // Keep comments – they're harmless and often useful.
                    if matches!(lines[j], Line::Comment(_) | Line::Empty) {
                        j += 1;
                        continue;
                    }
                    lines.remove(j);
                    changed = true;
                }
            }
        }
        i += 1;
    }
    changed
}

/// Rule 16 – Remove double XCHG (XCHG / XCHG → nothing).
///
/// XCHG swaps HL and DE.  Two consecutive XCHGs restore the original state.
fn rule_remove_double_xchg(lines: &mut Vec<Line>) -> bool {
    let mut changed = false;
    let mut i = 0;
    while i + 1 < lines.len() {
        if let (
            Line::Instruction { opcode: op_a, .. },
            Line::Instruction { opcode: op_b, .. },
        ) = (&lines[i], &lines[i + 1])
        {
            if op_a == "XCHG" && op_b == "XCHG" {
                lines.remove(i + 1);
                lines.remove(i);
                changed = true;
                continue;
            }
        }
        i += 1;
    }
    changed
}

/// Rule 24 – Remove double CMA (CMA / CMA → nothing).
///
/// CMA complements the accumulator.  Two consecutive CMAs restore the
/// original value.
fn rule_remove_double_cma(lines: &mut Vec<Line>) -> bool {
    let mut changed = false;
    let mut i = 0;
    while i + 1 < lines.len() {
        if let (
            Line::Instruction { opcode: op_a, .. },
            Line::Instruction { opcode: op_b, .. },
        ) = (&lines[i], &lines[i + 1])
        {
            if op_a == "CMA" && op_b == "CMA" {
                lines.remove(i + 1);
                lines.remove(i);
                changed = true;
                continue;
            }
        }
        i += 1;
    }
    changed
}

/// Rule 18 – Remove jump to the immediately following label.
///
/// `JMP L1` followed by `L1:` can be deleted since execution falls
/// through anyway.
fn rule_jump_to_next(lines: &mut Vec<Line>) -> bool {
    let mut changed = false;
    let mut i = 0;
    while i + 1 < lines.len() {
        if let Line::Instruction { opcode, operands } = &lines[i] {
            if opcode == "JMP" {
                // Skip over comments and empty lines to find the next label.
                let mut j = i + 1;
                while j < lines.len() && matches!(lines[j], Line::Comment(_) | Line::Empty) {
                    j += 1;
                }
                if j < lines.len() {
                    if let Line::Label(name) = &lines[j] {
                        if operands.trim() == name {
                            lines.remove(i);
                            changed = true;
                            continue;
                        }
                    }
                }
            }
        }
        i += 1;
    }
    changed
}

/// Rule 35 – Inline main (special case for CRT0 startup).
///
/// Replaces `CALL main` in `_start` with the body of `main` (excluding
/// trailing `RET`), then keeps `main:` around for compatibility.
fn rule_inline_main(lines: &mut Vec<Line>) -> bool {
    // Find main function body markers.
    let main_label_pos = match lines.iter().position(|line| match line {
        Line::Label(name) if name == "main" => true,
        _ => false,
    }) {
        Some(i) => i,
        None => return false,
    };

    // Find the first label after main that is not a local main label.
    let mut main_end = lines.len();
    for i in main_label_pos + 1..lines.len() {
        if let Line::Label(name) = &lines[i] {
            if !name.ends_with("__main") {
                main_end = i;
                break;
            }
        }
    }

    // Collect main body (skip trailing RET if present).
    let mut body: Vec<Line> = lines[main_label_pos + 1..main_end]
        .iter()
        .filter(|line| !matches!(line, Line::Empty))
        .cloned()
        .collect();

    if matches!(body.last(), Some(Line::Instruction { opcode, .. }) if opcode == "RET") {
        body.pop();
    }

    // Find the startup call to main.
    let call_idx = match lines.iter().position(|line| match line {
        Line::Instruction { opcode, operands } => {
            opcode == "CALL" && operands.trim() == "main"
        }
        _ => false,
    }) {
        Some(i) => i,
        None => return false,
    };

    // Insert body at call site; remove CALL main.
    lines.remove(call_idx);
    for (offset, line) in body.iter().cloned().enumerate() {
        lines.insert(call_idx + offset, line);
    }

    true
}

/// Rule 19 – Conditional branch inversion.
///
/// ```text
/// JZ  L1        JNZ L2
/// JMP L2   →
/// L1:           L1:
/// ```
///
/// Replaces a conditional-jump-over-unconditional-jump pattern with
/// the inverted condition, removing one instruction.
fn rule_branch_inversion(lines: &mut Vec<Line>) -> bool {
    let mut changed = false;
    let mut i = 0;
    while i < lines.len() {
        let Some(jmp_idx) = next_significant_line(lines, i + 1) else {
            break;
        };
        let Some(label_idx) = next_significant_line(lines, jmp_idx + 1) else {
            break;
        };

        let is_match = match (&lines[i], &lines[jmp_idx], &lines[label_idx]) {
            (
                Line::Instruction { opcode: op_cond, operands: lbl_cond },
                Line::Instruction { opcode: op_jmp, operands: lbl_jmp },
                Line::Label(next_label),
            ) if op_jmp == "JMP" && lbl_cond.trim() == next_label => {
                invert_condition(op_cond).map(|inverted| (inverted, lbl_jmp.clone(), jmp_idx))
            }
            _ => None,
        };

        if let Some((inverted_opcode, target, jmp_idx)) = is_match {
            lines[i] = Line::Instruction {
                opcode: inverted_opcode,
                operands: target,
            };
            lines.remove(jmp_idx);
            changed = true;
        }
        i += 1;
    }
    changed
}

/// Rule 33 - Remove conditional jumps to the immediately following label.
fn rule_conditional_jump_to_next(lines: &mut Vec<Line>) -> bool {
    let mut changed = false;
    let mut i = 0;
    while i + 1 < lines.len() {
        if let Line::Instruction { opcode, operands } = &lines[i] {
            if is_conditional_jump(opcode) {
                let mut j = i + 1;
                while j < lines.len() && matches!(lines[j], Line::Comment(_) | Line::Empty) {
                    j += 1;
                }
                if let Some(Line::Label(name)) = lines.get(j) {
                    if operands.trim() == name {
                        lines.remove(i);
                        changed = true;
                        continue;
                    }
                }
            }
        }
        i += 1;
    }
    changed
}

/// Rule 34 - Replace a jump to a label whose first instruction is RET.
fn rule_jump_to_ret(lines: &mut Vec<Line>) -> bool {
    let mut label_index: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for (idx, line) in lines.iter().enumerate() {
        if let Line::Label(name) = line {
            label_index.insert(name.clone(), idx);
        }
    }

    let mut changed = false;
    let mut replacements: Vec<(usize, Line)> = Vec::new();
    for i in 0..lines.len() {
        let replacement = match &lines[i] {
            Line::Instruction { opcode, operands } if opcode == "JMP" => {
                let target = operands.trim();
                let Some(&label_idx) = label_index.get(target) else {
                    continue;
                };
                let Some(next_idx) = next_significant_line(lines, label_idx + 1) else {
                    continue;
                };
                match &lines[next_idx] {
                    Line::Instruction { opcode, .. } if opcode == "RET" => Some(Line::Instruction {
                        opcode: "RET".to_string(),
                        operands: String::new(),
                    }),
                    _ => None,
                }
            }
            _ => None,
        };

        if let Some(new_line) = replacement {
            replacements.push((i, new_line));
            changed = true;
        }
    }

    for (idx, line) in replacements {
        lines[idx] = line;
    }
    changed
}

/// Rule 21 – Replace MVI A,0 with XRA A.
///
/// XRA A is 1 byte vs MVI A,0 which is 2 bytes; both set A to 0.
fn rule_mvi_a_zero_to_xra(lines: &mut Vec<Line>) -> bool {
    let mut changed = false;
    for line in lines.iter_mut() {
        if let Line::Instruction { opcode, operands } = line {
            if opcode == "MVI" && operands.trim() == "A,0" {
                *opcode = "XRA".to_string();
                *operands = "A".to_string();
                changed = true;
            }
        }
    }
    changed
}

/// Rule 25/26 – Remove INX/DCX pairs that cancel each other.
///
/// INX H / DCX H → nothing (and vice versa).
fn rule_remove_inx_dcx_pairs(lines: &mut Vec<Line>) -> bool {
    let mut changed = false;
    let mut i = 0;
    while i + 1 < lines.len() {
        if let (
            Line::Instruction { opcode: op_a, operands: reg_a },
            Line::Instruction { opcode: op_b, operands: reg_b },
        ) = (&lines[i], &lines[i + 1])
        {
            let cancel = (op_a == "INX" && op_b == "DCX" && reg_a.trim() == reg_b.trim())
                || (op_a == "DCX" && op_b == "INX" && reg_a.trim() == reg_b.trim());
            if cancel {
                lines.remove(i + 1);
                lines.remove(i);
                changed = true;
                continue;
            }
        }
        i += 1;
    }
    changed
}

/// Rule 28/29 – Remove redundant LDA/STA pairs.
///
/// LDA addr / STA addr → just LDA addr (store after load to same address).
/// STA addr / LDA addr → just STA addr (load after store to same address).
fn rule_lda_sta_pairs(lines: &mut Vec<Line>) -> bool {
    let mut changed = false;
    let mut i = 0;
    while i + 1 < lines.len() {
        if let (
            Line::Instruction { opcode: op_a, operands: addr_a },
            Line::Instruction { opcode: op_b, operands: addr_b },
        ) = (&lines[i], &lines[i + 1])
        {
            if addr_a == addr_b
                && ((op_a == "LDA" && op_b == "STA") || (op_a == "STA" && op_b == "LDA"))
            {
                lines.remove(i + 1);
                changed = true;
                continue;
            }
        }
        i += 1;
    }
    changed
}

/// Rule 32 – Jump threading.
///
/// When a jump (conditional or unconditional) targets a label that is
/// immediately followed by an unconditional jump, rewrite the original jump
/// to target the final destination directly.  This eliminates chains of
/// jumps:
///
/// ```text
/// JMP L1        JMP L2
/// ...      →    ...
/// L1:           L1:
/// JMP L2        JMP L2
/// ```
///
/// Also handles conditional jumps: `JZ L1` where `L1: JMP L2` → `JZ L2`.
fn rule_jump_threading(lines: &mut Vec<Line>) -> bool {
    // Build a map: label → index in `lines`.
    let mut label_index: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for (i, line) in lines.iter().enumerate() {
        if let Line::Label(name) = line {
            label_index.insert(name.as_str(), i);
        }
    }

    // For each label, find the first non-label, non-comment, non-empty
    // instruction after it.  If that instruction is `JMP target`, record
    // the forwarding.
    let mut forward: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for (name, &idx) in &label_index {
        let mut j = idx + 1;
        while j < lines.len() {
            match &lines[j] {
                Line::Label(_) | Line::Comment(_) | Line::Empty => { j += 1; }
                Line::Instruction { opcode, operands } => {
                    if opcode == "JMP" {
                        forward.insert((*name).to_string(), operands.trim().to_string());
                    }
                    break;
                }
            }
        }
    }

    if forward.is_empty() {
        return false;
    }

    // Resolve transitive chains: L1 → L2 → L3 becomes L1 → L3.
    // Limit iteration to prevent infinite loops on cycles.
    for _ in 0..16 {
        let mut any = false;
        let snapshot: Vec<(String, String)> = forward.iter().map(|(k,v)| (k.clone(), v.clone())).collect();
        for (src, dst) in &snapshot {
            if let Some(further) = forward.get(dst) {
                if further != src {
                    forward.insert(src.clone(), further.clone());
                    any = true;
                }
            }
        }
        if !any { break; }
    }

    // Rewrite jump targets.
    let mut changed = false;
    for line in lines.iter_mut() {
        if let Line::Instruction { opcode, operands } = line {
            let is_jump = opcode == "JMP"
                || opcode == "JZ" || opcode == "JNZ"
                || opcode == "JC" || opcode == "JNC"
                || opcode == "JM" || opcode == "JP"
                || opcode == "JPE" || opcode == "JPO";
            if is_jump {
                let target = operands.trim();
                if let Some(final_target) = forward.get(target) {
                    if final_target != target {
                        *operands = final_target.clone();
                        changed = true;
                    }
                }
            }
        }
    }

    changed
}

/// Rule 31 – Remove unreferenced compiler-generated labels.
///
/// Internal labels (starting with `L`, `__cg_`, or `__cmp_done_`) that are
/// never referenced in any operand can be safely removed.
/// Rules 37 & 38 – Remove redundant HL loads.
///
/// **Rule 37** – `LXI H,N` when HL already holds N:
///   A second `LXI H,N` is dead if nothing has changed HL since the last
///   `LXI H,N` or `SHLD addr` (which leaves HL intact).
///
/// **Rule 38** – `LHLD addr` when HL already holds the content of `addr`:
///   After `SHLD addr` (and any number of HL-preserving instructions),
///   a subsequent `LHLD addr` is dead because HL still holds the same value
///   that was stored there.  This commonly occurs when regalloc spills a
///   local variable via `SHLD` and then reloads it with `LHLD` even though
///   HL was never overwritten in between.
///
/// Rule 43 — `LXI H,N / MOV E,M / INX H / MOV D,M / XCHG` → `LHLD N`.
///
/// When a 16-bit big-little-endian load is performed via HL at a literal
/// address N, the five-instruction sequence is equivalent to one `LHLD N`
/// instruction (L ← (N), H ← (N+1)).  This is the safety-net counterpart
/// to the codegen fast path in gen_load_ptr; it also fires for any such
/// patterns left over by other transformations.
fn rule_lxi_w16_load_to_lhld(lines: &mut Vec<Line>) -> bool {
    let mut changed = false;
    let mut i = 0;
    while i + 4 < lines.len() {
        // Collect the next 5 items; any of them may be a label or blank.
        // We need 5 consecutive *instructions* (no intervening labels).
        let window: Vec<&Line> = lines[i..i + 5].iter().collect();
        let instrs: Vec<(&str, &str)> = window
            .iter()
            .filter_map(|l| {
                if let Line::Instruction { opcode, operands } = l {
                    Some((opcode.as_str(), operands.trim()))
                } else {
                    None
                }
            })
            .collect();

        if instrs.len() == 5 {
            let matches = instrs[0].0 == "LXI"  && instrs[0].1.starts_with("H,")
                       && instrs[1] == ("MOV", "E,M")
                       && instrs[2] == ("INX", "H")
                       && instrs[3] == ("MOV", "D,M")
                       && instrs[4] == ("XCHG", "");
            if matches {
                let addr = instrs[0].1["H,".len()..].to_string();
                lines.splice(i..i + 5, [Line::Instruction {
                    opcode: "LHLD".to_string(),
                    operands: format!(" {}", addr),
                }]);
                changed = true;
                continue;
            }
        }
        i += 1;
    }
    changed
}

/// HL-clobbering instructions (clear all HL state):
///   MOV H/L, MVI H/L, DAD, INX H, DCX H, XCHG, POP H,
///   any CALL/return opcode, and labels (unknown HL state at join points).
///   LHLD is handled explicitly below (it sets a new `hl_from_addr`).
fn rule_elim_redundant_lxi_h(lines: &mut Vec<Line>) -> bool {
    let mut changed = false;
    let mut hl_numeric: Option<String> = None;   // HL = this immediate (from LXI H,N)
    let mut hl_from_addr: Option<String> = None; // HL = content-of(this label) (from SHLD/LHLD)
    let mut i = 0;
    while i < lines.len() {
        match &lines[i] {
            Line::Label(_) => {
                // Control-flow join point — HL state is unknown.
                hl_numeric = None;
                hl_from_addr = None;
            }
            Line::Instruction { opcode, operands } => {
                let opcode = opcode.clone();
                let operands = operands.clone();
                let ops = operands.trim();
                if opcode == "LXI" && ops.starts_with("H,") {
                    let imm = ops["H,".len()..].trim();
                    if hl_numeric.as_deref() == Some(imm) {
                        // Rule 37: HL already holds this immediate — drop LXI.
                        lines.remove(i);
                        changed = true;
                        continue;
                    }
                    hl_numeric = Some(imm.to_string());
                    hl_from_addr = None; // New numeric value, not yet stored anywhere.
                } else if opcode == "SHLD" {
                    // SHLD stores HL to memory but does NOT change HL.
                    // Record that HL now mirrors the content of `ops`.
                    hl_from_addr = Some(ops.to_string());
                    // hl_numeric remains valid (HL value is unchanged).
                } else if opcode == "LHLD" {
                    if hl_from_addr.as_deref() == Some(ops) {
                        // Rule 38: HL already holds the content of `ops` — drop LHLD.
                        lines.remove(i);
                        changed = true;
                        continue;
                    }
                    // HL is now loaded from memory; numeric value unknown.
                    hl_numeric = None;
                    hl_from_addr = Some(ops.to_string());
                } else if opcode == "STA" && hl_from_addr.as_deref() == Some(ops) {
                    // A byte-level write to our tracked label may corrupt the
                    // low byte of the stored 16-bit value — invalidate.
                    hl_from_addr = None;
                } else if clobbers_hl_value(&opcode, ops) {
                    hl_numeric = None;
                    hl_from_addr = None;
                }
                // Other instructions do not affect HL — both trackers stay valid.
            }
            _ => {}
        }
        i += 1;
    }
    changed
}

/// Returns `true` if the instruction may write to the HL register pair.
///
/// Conservative: when in doubt, return `true` (clear tracking) to avoid
/// misoptimizations.
fn clobbers_hl_value(opcode: &str, ops: &str) -> bool {
    match opcode {
        // Note: LHLD is handled explicitly in rule_elim_redundant_lxi_h;
        // it is NOT listed here to avoid double-clearing the hl_from_addr state.
        // Partial writes to H or L
        "MOV" | "MVI" => ops.starts_with("H,") || ops.starts_with("L,"),
        // Double-add writes result back into HL
        "DAD" => true,
        // Increment / decrement HL
        "INX" | "DCX" => ops == "H",
        // Swap HL ↔ DE
        "XCHG" => true,
        // Pop into HL
        "POP" => ops == "H",
        // Calls: callee may clobber HL
        "CALL" | "CZ" | "CNZ" | "CC" | "CNC" | "CPE" | "CPO" | "CM" | "CP" => true,
        // Returns: HL state at the call site is unknown after a potential call
        // (this branch is never reached in a straight-line sequence, but keep
        // for completeness).
        "RET" | "RZ" | "RNZ" | "RC" | "RNC" | "RPE" | "RPO" | "RM" | "RP" => true,
        _ => false,
    }
}

/// Rule 40 – Remove dead spill stores.
///
/// `SHLD __spill_N` and `STA __spill_N` are generated by the register
/// allocator to save a live value before overwriting the register.  After
/// peephole rules eliminate many of the corresponding `LHLD __spill_N` /
/// `LDA __spill_N` reloads, the store itself is often left with no reader and
/// can be removed.
///
/// A spill store is dead when the spill label (`__spill_N`) is never read
/// (via `LHLD` or `LDA`) anywhere in the current function's instruction list.
/// We only touch labels that start with `__spill_` to avoid removing
/// legitimate user-visible globals.
fn rule_elim_dead_spills(lines: &mut Vec<Line>) -> bool {
    // Collect every compiler-local label that is READ (load operations).
    // A label is compiler-local if it starts with `__spill_` (a register-spill
    // slot) or `_l_` (a function-local variable).
    let mut live: std::collections::HashSet<String> =
        std::collections::HashSet::new();
    for line in lines.iter() {
        if let Line::Instruction { opcode, operands } = line {
            if matches!(opcode.as_str(), "LHLD" | "LDA" | "LDAX") {
                let label = operands.trim();
                if is_compiler_local_label(label) {
                    live.insert(label.to_string());
                }
            }
            // LXI H,label followed by MOV r,M is a byte-load pattern emitted
            // for W8 spill reloads into pair registers.  Treat the label as read.
            if opcode == "LXI" {
                let operands = operands.trim();
                if let Some(rest) = operands.strip_prefix("H,") {
                    let label = rest.trim();
                    if is_compiler_local_label(label) {
                        live.insert(label.to_string());
                    }
                }
            }
        }
    }
    // Remove write-only stores to compiler-local labels.
    let before = lines.len();
    lines.retain(|line| {
        if let Line::Instruction { opcode, operands } = line {
            if matches!(opcode.as_str(), "SHLD" | "STA") {
                let label = operands.trim();
                if is_compiler_local_label(label) && !live.contains(label) {
                    return false;
                }
            }
        }
        true
    });
    lines.len() != before
}

/// Rule 41 – Remove unused compiler-local `.STORAGE` declarations.
///
/// After Rule 40 removes all dead SHLD/STA stores, and Rule 38 removes
/// redundant LHLD reloads, a compiler-local label (`_l_*` or `__spill_*`)
/// may be completely unreferenced in any instruction operand.  In that case
/// the label declaration and its `.STORAGE N` are wasted bytes and can be
/// removed.
///
/// A label is considered referenced if it (or a `label+offset` variant)
/// appears in any instruction operand anywhere in the file.
fn rule_elim_unused_storage(lines: &mut Vec<Line>) -> bool {
    // Collect all label strings that appear in instruction operands.
    let mut referenced: std::collections::HashSet<String> =
        std::collections::HashSet::new();
    for line in lines.iter() {
        if let Line::Instruction { operands, .. } = line {
            let trimmed = operands.trim();
            if !trimmed.is_empty() {
                referenced.insert(trimmed.to_string());
            }
        }
    }

    // Helper: returns true if `name` is referenced as itself or as `name+N`.
    let is_referenced = |name: &str| {
        referenced.contains(name)
            || referenced
                .iter()
                .any(|r| r.starts_with(&format!("{}+", name)))
    };

    // Walk lines and drop [Label + .STORAGE] pairs for unreferenced
    // compiler-local labels.
    let mut to_remove: std::collections::HashSet<usize> =
        std::collections::HashSet::new();
    let mut i = 0;
    while i < lines.len() {
        if let Line::Label(name) = &lines[i] {
            if is_compiler_local_label(name) && !is_referenced(name) {
                // Look ahead: if the next non-empty/non-comment line is .STORAGE, kill both.
                let mut j = i + 1;
                while j < lines.len() {
                    match &lines[j] {
                        Line::Comment(_) | Line::Empty => { j += 1; }
                        Line::Instruction { opcode, .. }
                            if opcode == ".STORAGE" || opcode == "DS" =>
                        {
                            to_remove.insert(i);
                            to_remove.insert(j);
                            break;
                        }
                        _ => break,
                    }
                }
            }
        }
        i += 1;
    }

    if to_remove.is_empty() {
        return false;
    }
    let mut idx = 0;
    lines.retain(|_| {
        let keep = !to_remove.contains(&idx);
        idx += 1;
        keep
    });
    true
}

/// Returns `true` if the label is a compiler-generated local (safe to
/// eliminate when unused): register-spill slots or function-local variables.
fn is_compiler_local_label(name: &str) -> bool {
    name.starts_with("__spill_") || name.starts_with("_l_")
}

fn rule_remove_unreferenced_labels(lines: &mut Vec<Line>) -> bool {
    // Collect all operand references.
    let mut referenced: std::collections::HashSet<String> = std::collections::HashSet::new();
    for line in lines.iter() {
        if let Line::Instruction { operands, .. } = line {
            let trimmed = operands.trim();
            if !trimmed.is_empty() {
                referenced.insert(trimmed.to_string());
            }
        }
    }

    let before = lines.len();
    lines.retain(|line| {
        if let Line::Label(name) = line {
            // Only remove compiler-generated labels; keep user labels and
            // function entry points (which don't start with L or __cg_).
            if is_compiler_label(name) && !referenced.contains(name) {
                return false;
            }
        }
        true
    });
    lines.len() != before
}

/// Returns `true` if the label name looks like a compiler-generated internal label
/// that is safe to remove when unreferenced.
fn is_compiler_label(name: &str) -> bool {
    // Only remove __cg_ labels (codegen temporaries for comparisons etc.)
    // Do NOT remove L0, L1, etc. (IR labels) as they may be jump targets
    // that were already optimized away.
    name.starts_with("__cg_")
        || name.starts_with("__cmp_done_")
}

/// Invert a conditional jump opcode.
fn invert_condition(opcode: &str) -> Option<String> {
    match opcode {
        "JZ" => Some("JNZ".to_string()),
        "JNZ" => Some("JZ".to_string()),
        "JC" => Some("JNC".to_string()),
        "JNC" => Some("JC".to_string()),
        "JM" => Some("JP".to_string()),
        "JP" => Some("JM".to_string()),
        "JPE" => Some("JPO".to_string()),
        "JPO" => Some("JPE".to_string()),
        _ => None,
    }
}

fn next_significant_line(lines: &[Line], mut start: usize) -> Option<usize> {
    while start < lines.len() {
        match lines[start] {
            Line::Comment(_) | Line::Empty => start += 1,
            _ => return Some(start),
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Optimize a sequence of Intel 8080 assembly lines using peephole rules.
///
/// The optimizer parses each line into a structured form, then repeatedly
/// applies pattern-matched rewrite rules until no rule fires (fixed-point
/// iteration).  The optimized lines are returned as plain strings.
///
/// # Example
///
/// ```ignore
/// let asm = vec![
///     "\tSHLD _x".to_string(),
///     "\tLHLD _x".to_string(),
/// ];
/// let opt = peephole_optimize(asm);
/// assert_eq!(opt, vec!["\tSHLD _x"]);
/// ```
pub fn peephole_optimize(lines: Vec<String>) -> Vec<String> {
    let mut parsed: Vec<Line> = lines.iter().map(|s| parse_line(s)).collect();

    loop {
        if !apply_rules(&mut parsed) {
            break;
        }
    }

    parsed.iter().map(render_line).collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Convenience: build a `Vec<String>` from string slices.
    fn asm(lines: &[&str]) -> Vec<String> {
        lines.iter().map(|s| s.to_string()).collect()
    }

    // -- Rule 1: Remove redundant load after store ------------------------

    #[test]
    fn rule1_shld_lhld_same_addr() {
        let input = asm(&["\tSHLD _x", "\tLHLD _x"]);
        let out = peephole_optimize(input);
        assert_eq!(out, asm(&["\tSHLD _x"]));
    }

    #[test]
    fn rule1_different_addr_preserved() {
        let input = asm(&["\tSHLD _x", "\tLHLD _y"]);
        let out = peephole_optimize(input);
        assert_eq!(out, asm(&["\tSHLD _x", "\tLHLD _y"]));
    }

    // -- Rule 2: Remove redundant store after load ------------------------

    #[test]
    fn rule2_lhld_shld_same_addr() {
        let input = asm(&["\tLHLD _x", "\tSHLD _x"]);
        let out = peephole_optimize(input);
        assert_eq!(out, asm(&["\tLHLD _x"]));
    }

    // -- Rule 3: Tail-call optimisation -----------------------------------

    #[test]
    fn rule3_call_ret_becomes_jmp() {
        let input = asm(&["\tCALL _puts", "\tRET"]);
        let out = peephole_optimize(input);
        assert_eq!(out, asm(&["\tJMP _puts"]));
    }

    #[test]
    fn rule3_call_without_ret_unchanged() {
        let input = asm(&["\tCALL _puts", "\tMOV A,B"]);
        let out = peephole_optimize(input);
        assert_eq!(out, asm(&["\tCALL _puts", "\tMOV A,B"]));
    }

    // -- Rule 4: Remove self-move -----------------------------------------

    #[test]
    fn rule4_self_move_deleted() {
        let input = asm(&["\tMOV A,A", "\tMOV H,H", "\tMOV A,B"]);
        let out = peephole_optimize(input);
        assert_eq!(out, asm(&["\tMOV A,B"]));
    }

    // -- Rule 5: Remove redundant CPI 0 after ORA ------------------------

    #[test]
    fn rule5_ora_cpi_zero() {
        let input = asm(&["\tORA L", "\tCPI 0", "\tJZ L1"]);
        let out = peephole_optimize(input);
        assert_eq!(out, asm(&["\tORA L", "\tJZ L1"]));
    }

    #[test]
    fn rule5_ora_cpi_nonzero_kept() {
        let input = asm(&["\tORA L", "\tCPI 1"]);
        let out = peephole_optimize(input);
        assert_eq!(out, asm(&["\tORA L", "\tCPI 1"]));
    }

    // -- Rule 6: LXI H,0 already optimal ---------------------------------

    #[test]
    fn rule6_lxi_h_zero_unchanged() {
        let input = asm(&["\tLXI H,0"]);
        let out = peephole_optimize(input);
        assert_eq!(out, asm(&["\tLXI H,0"]));
    }

    // -- Rule 7: Duplicate LHLD -------------------------------------------

    #[test]
    fn rule7_duplicate_lhld() {
        let input = asm(&["\tLHLD _x", "\tLHLD _x"]);
        let out = peephole_optimize(input);
        assert_eq!(out, asm(&["\tLHLD _x"]));
    }

    // -- Rule 8: Duplicate SHLD -------------------------------------------

    #[test]
    fn rule8_duplicate_shld() {
        let input = asm(&["\tSHLD _x", "\tSHLD _x"]);
        let out = peephole_optimize(input);
        assert_eq!(out, asm(&["\tSHLD _x"]));
    }

    // -- Rule 9: INX H after LHLD (keep both, recognize pattern) ----------

    #[test]
    fn rule9_inx_after_lhld_preserved() {
        let input = asm(&["\tLHLD _arr", "\tINX H"]);
        let out = peephole_optimize(input);
        assert_eq!(out, asm(&["\tLHLD _arr", "\tINX H"]));
    }

    // -- Rule 10: Dead code after JMP -------------------------------------

    #[test]
    fn rule10_dead_code_after_jmp() {
        let input = asm(&["\tJMP L1", "\tMOV A,B", "\tADD C", "L1:"]);
        let out = peephole_optimize(input);
        // Dead code removed, then JMP L1 to immediately-following L1: also removed.
        assert_eq!(out, asm(&["L1:"]));
    }

    #[test]
    fn rule10_comments_preserved_after_jmp() {
        let input = asm(&["\tJMP L1", "; comment", "\tMOV A,B", "L1:"]);
        let out = peephole_optimize(input);
        // Dead code (MOV A,B) removed, then JMP L1 to next L1: also removed.
        assert_eq!(out, asm(&["; comment", "L1:"]));
    }

    #[test]
    fn rule10_label_stops_deletion() {
        let input = asm(&["\tJMP L1", "L2:", "\tMOV A,B", "L1:"]);
        let out = peephole_optimize(input);
        assert_eq!(out, asm(&["\tJMP L1", "L2:", "\tMOV A,B", "L1:"]));
    }

    #[test]
    fn rule18_conditional_jump_to_next_label_removed() {
        let input = asm(&["\tJZ L1", "; note", "L1:", "\tRET"]);
        let out = peephole_optimize(input);
        assert_eq!(out, asm(&["; note", "L1:", "\tRET"]));
    }

    #[test]
    fn rule19_branch_inversion_skips_comments() {
        let input = asm(&["\tJZ L1", "; mid", "\tJMP L2", "L1:"]);
        let out = peephole_optimize(input);
        assert_eq!(out, asm(&["\tJNZ L2", "; mid", "L1:"]));
    }

    #[test]
    fn rule34_jump_to_ret_becomes_ret() {
        let input = asm(&["\tJMP done", "done:", "\tRET"]);
        let out = peephole_optimize(input);
        assert_eq!(out, asm(&["done:", "\tRET"]));
    }

    // -- Rule 11: LXI H,0 / DAD SP kept as-is ----------------------------

    #[test]
    fn rule11_lxi_dad_sp_unchanged() {
        let input = asm(&["\tLXI H,0", "\tDAD SP"]);
        let out = peephole_optimize(input);
        assert_eq!(out, asm(&["\tLXI H,0", "\tDAD SP"]));
    }

    // -- Rule 12: PUSH / POP same register --------------------------------

    #[test]
    fn rule12_push_pop_same_deleted() {
        let input = asm(&["\tPUSH H", "\tPOP H"]);
        let out = peephole_optimize(input);
        assert!(out.is_empty());
    }

    #[test]
    fn rule12_push_pop_different_kept() {
        let input = asm(&["\tPUSH H", "\tPOP D"]);
        let out = peephole_optimize(input);
        assert_eq!(out, asm(&["\tPUSH H", "\tPOP D"]));
    }

    // -- Rule 13: Merge adjacent labels -----------------------------------

    #[test]
    fn rule13_merge_adjacent_labels() {
        let input = asm(&["L1:", "L2:", "\tJMP L2"]);
        let out = peephole_optimize(input);
        assert_eq!(out, asm(&["L1:", "\tJMP L1"]));
    }

    #[test]
    fn rule13_three_adjacent_labels() {
        let input = asm(&["L1:", "L2:", "L3:", "\tJMP L3"]);
        let out = peephole_optimize(input);
        assert_eq!(out, asm(&["L1:", "\tJMP L1"]));
    }

    // -- Rule 14: 16-bit zero test pattern (keep as-is) -------------------

    #[test]
    fn rule14_16bit_zero_test_preserved() {
        let input = asm(&["\tMOV A,H", "\tORA L", "\tJZ L1"]);
        let out = peephole_optimize(input);
        assert_eq!(out, asm(&["\tMOV A,H", "\tORA L", "\tJZ L1"]));
    }

    // -- Rule 15: Remove NOPs --------------------------------------------

    #[test]
    fn rule15_nop_removed() {
        let input = asm(&["\tNOP", "\tMOV A,B", "\tNOP"]);
        let out = peephole_optimize(input);
        assert_eq!(out, asm(&["\tMOV A,B"]));
    }

    // -- Fixed-point iteration -------------------------------------------

    #[test]
    fn fixed_point_multi_pass() {
        // First pass: SHLD/LHLD collapses, then the duplicate SHLD collapses.
        let input = asm(&["\tSHLD _x", "\tLHLD _x", "\tSHLD _x"]);
        let out = peephole_optimize(input);
        assert_eq!(out, asm(&["\tSHLD _x"]));
    }

    #[test]
    fn fixed_point_push_pop_chain() {
        // Two consecutive PUSH/POP pairs.
        let input = asm(&["\tPUSH H", "\tPOP H", "\tPUSH D", "\tPOP D"]);
        let out = peephole_optimize(input);
        assert!(out.is_empty());
    }

    // -- Misc: labels, comments, empty lines preserved --------------------

    #[test]
    fn labels_and_comments_preserved() {
        let input = asm(&["main:", "; entry point", "\tRET", ""]);
        let out = peephole_optimize(input);
        assert_eq!(out, asm(&["main:", "; entry point", "\tRET", ""]));
    }

    #[test]
    fn empty_input() {
        let out = peephole_optimize(vec![]);
        assert!(out.is_empty());
    }

    // -- Combined rules ---------------------------------------------------

    #[test]
    fn combined_tail_call_and_dead_code() {
        let input = asm(&[
            "\tCALL _func",
            "\tRET",
            "\tMOV A,B",  // dead after the JMP that replaces CALL/RET
            "next:",
        ]);
        let out = peephole_optimize(input);
        assert_eq!(out, asm(&["\tJMP _func", "next:"]));
    }

    #[test]
    fn combined_nop_and_self_move() {
        let input = asm(&["\tNOP", "\tMOV A,A", "\tMOV A,B", "\tNOP"]);
        let out = peephole_optimize(input);
        assert_eq!(out, asm(&["\tMOV A,B"]));
    }

    // -- Rule 16: Double XCHG removal ------------------------------------

    #[test]
    fn rule16_double_xchg_removed() {
        let input = asm(&["\tXCHG", "\tXCHG"]);
        let out = peephole_optimize(input);
        assert!(out.is_empty());
    }

    #[test]
    fn rule16_single_xchg_kept() {
        let input = asm(&["\tXCHG", "\tMOV A,B"]);
        let out = peephole_optimize(input);
        assert_eq!(out, asm(&["\tXCHG", "\tMOV A,B"]));
    }

    // -- Rule 18: Jump to next label removal -----------------------------

    #[test]
    fn rule18_jump_to_next_label_removed() {
        let input = asm(&["\tJMP L1", "L1:", "\tRET"]);
        let out = peephole_optimize(input);
        // JMP L1 should be removed; L1 may also be removed if unreferenced
        assert!(has_line(&out, "RET"));
        assert!(!has_line(&out, "JMP L1"));
    }

    #[test]
    fn rule18_jump_to_next_with_comment() {
        let input = asm(&["\tJMP L1", "; comment", "L1:", "\tRET"]);
        let out = peephole_optimize(input);
        assert!(!has_line(&out, "JMP L1"));
    }

    #[test]
    fn rule18_jump_to_different_label_kept() {
        let input = asm(&["\tJMP L2", "L1:", "\tRET"]);
        let out = peephole_optimize(input);
        assert!(has_line(&out, "JMP L2"));
    }

    // -- Rule 19: Conditional branch inversion ---------------------------

    #[test]
    fn rule19_branch_inversion_jz() {
        let input = asm(&["\tJZ L1", "\tJMP L2", "L1:", "\tRET"]);
        let out = peephole_optimize(input);
        assert!(has_line(&out, "JNZ L2"));
        assert!(!has_line(&out, "JZ L1"));
    }

    #[test]
    fn rule19_branch_inversion_jnz() {
        let input = asm(&["\tJNZ L1", "\tJMP L2", "L1:", "\tRET"]);
        let out = peephole_optimize(input);
        assert!(has_line(&out, "JZ L2"));
    }

    #[test]
    fn rule19_branch_inversion_jc() {
        let input = asm(&["\tJC L1", "\tJMP L2", "L1:", "\tRET"]);
        let out = peephole_optimize(input);
        assert!(has_line(&out, "JNC L2"));
    }

    #[test]
    fn rule19_branch_inversion_jm() {
        let input = asm(&["\tJM L1", "\tJMP L2", "L1:", "\tRET"]);
        let out = peephole_optimize(input);
        assert!(has_line(&out, "JP L2"));
    }

    // -- Rule 21: MVI A,0 → XRA A ---------------------------------------

    #[test]
    fn rule21_mvi_a_zero_becomes_xra() {
        let input = asm(&["\tMVI A,0"]);
        let out = peephole_optimize(input);
        assert_eq!(out, asm(&["\tXRA A"]));
    }

    #[test]
    fn rule21_mvi_a_nonzero_kept() {
        let input = asm(&["\tMVI A,1"]);
        let out = peephole_optimize(input);
        assert_eq!(out, asm(&["\tMVI A,1"]));
    }

    // -- Rule 24: Double CMA removal -------------------------------------

    #[test]
    fn rule24_double_cma_removed() {
        let input = asm(&["\tCMA", "\tCMA"]);
        let out = peephole_optimize(input);
        assert!(out.is_empty());
    }

    #[test]
    fn rule24_single_cma_kept() {
        let input = asm(&["\tCMA", "\tMOV A,B"]);
        let out = peephole_optimize(input);
        assert_eq!(out, asm(&["\tCMA", "\tMOV A,B"]));
    }

    // -- Rule 25/26: INX/DCX pair removal --------------------------------

    #[test]
    fn rule25_inx_dcx_removed() {
        let input = asm(&["\tINX H", "\tDCX H"]);
        let out = peephole_optimize(input);
        assert!(out.is_empty());
    }

    #[test]
    fn rule26_dcx_inx_removed() {
        let input = asm(&["\tDCX H", "\tINX H"]);
        let out = peephole_optimize(input);
        assert!(out.is_empty());
    }

    #[test]
    fn rule25_different_regs_kept() {
        let input = asm(&["\tINX H", "\tDCX D"]);
        let out = peephole_optimize(input);
        assert_eq!(out, asm(&["\tINX H", "\tDCX D"]));
    }

    // -- Rule 28/29: LDA/STA pair removal --------------------------------

    #[test]
    fn rule28_lda_sta_same_addr() {
        let input = asm(&["\tLDA _x", "\tSTA _x"]);
        let out = peephole_optimize(input);
        assert_eq!(out, asm(&["\tLDA _x"]));
    }

    #[test]
    fn rule29_sta_lda_same_addr() {
        let input = asm(&["\tSTA _x", "\tLDA _x"]);
        let out = peephole_optimize(input);
        assert_eq!(out, asm(&["\tSTA _x"]));
    }

    #[test]
    fn rule28_different_addr_kept() {
        let input = asm(&["\tLDA _x", "\tSTA _y"]);
        let out = peephole_optimize(input);
        assert_eq!(out, asm(&["\tLDA _x", "\tSTA _y"]));
    }

    // -- Rule 31: Unreferenced label removal -----------------------------

    #[test]
    fn rule31_unreferenced_label_removed() {
        let input = asm(&["__cg_0:", "\tRET"]);
        let out = peephole_optimize(input);
        // __cg_0 is never referenced in any operand, so it should be removed
        assert_eq!(out, asm(&["\tRET"]));
    }

    #[test]
    fn rule31_referenced_label_kept() {
        let input = asm(&["\tJMP L0", "L0:", "\tRET"]);
        let out = peephole_optimize(input);
        assert!(has_line(&out, "L0:"));
    }

    #[test]
    fn rule31_function_label_kept() {
        let input = asm(&["main:", "\tRET"]);
        let out = peephole_optimize(input);
        assert_eq!(out, asm(&["main:", "\tRET"]));
    }

    // -- Combined new rules ----------------------------------------------

    #[test]
    fn combined_branch_inversion_and_dead_code() {
        let input = asm(&[
            "\tJZ L1",
            "\tJMP L2",
            "L1:",
            "\tJMP L3",
            "\tMOV A,B",  // dead code
            "L2:",
            "\tRET",
        ]);
        let out = peephole_optimize(input);
        assert!(has_line(&out, "JNZ L2"));
        assert!(!has_line(&out, "MOV A,B"));
    }

    // -- Helper ----------------------------------------------------------

    fn has_line(output: &[String], needle: &str) -> bool {
        output.iter().any(|l| l.contains(needle))
    }
}
