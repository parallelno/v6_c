//! Assembly text emitter.
//!
//! Writes the final `.asm` file for the **v6asm** assembler targeting
//! the Vector 06 computer.  The emitter prepends the standard header
//! (`ORG`, entry-point jump, CRT0 startup) and appends the runtime
//! library routines that are actually referenced by the generated code.

use std::io::Write;

use crate::runtime;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Write a complete `.asm` file to `output_path`.
///
/// The file contains, in order:
///
/// 1. A header comment identifying the compiler.
/// 2. `ORG 0x100` – the standard Vector 06 load address.
/// 3. `JMP _start` – jump to runtime startup.
/// 4. CRT0   – stack init, call to main, HLT.
/// 5. All assembly lines produced by the code generator / peephole optimizer.
/// 6. Referenced runtime library routines (auto-detected from CALL/JMP).
pub fn emit_asm(code_lines: &[String], output_path: &str) -> std::io::Result<()> {
    let mut f = std::fs::File::create(output_path)?;
    for line in build_full_asm(code_lines) {
        writeln!(f, "{}", line)?;
    }

    Ok(())
}

/// Write a human-readable `.lst` listing file to `output_path`.
///
/// The listing mirrors the final emitted assembly text and prefixes each line
/// with a 1-based line number.
pub fn emit_lst(
    code_lines: &[String],
    output_path: &str,
    source_lines: Option<&[String]>,
) -> std::io::Result<()> {
    let mut f = std::fs::File::create(output_path)?;
    writeln!(f, "ADDR   BYTES                    SOURCE")?;

    let asm_lines = build_full_asm(code_lines);
    let rows = build_listing_rows(&asm_lines, source_lines);
    for row in rows {
        let addr_col = row
            .address
            .map(|a| format!("{:04X}", a))
            .unwrap_or_else(|| "    ".to_string());
        let bytes_col = format_bytes_column(&row.bytes);
        writeln!(f, "{:<6} {:<24} {}", addr_col, bytes_col, row.text)?;
    }

    Ok(())
}

#[derive(Clone, Debug)]
struct ListingRow {
    address: Option<u16>,
    bytes: Vec<u8>,
    text: String,
}

fn build_listing_rows(lines: &[String], source_lines: Option<&[String]>) -> Vec<ListingRow> {
    let mut labels = std::collections::HashMap::new();
    let mut pc: u16 = 0;

    // Pass 1: resolve label addresses.
    for line in lines {
        let parsed = parse_listing_line(line);
        if let Some(label) = parsed.label {
            labels.insert(label, pc);
        }
        if let Some(stmt) = parsed.stmt {
            pc = pc.wrapping_add(stmt.size(&labels) as u16);
            if let ListingStmt::Org(v) = stmt {
                pc = v;
            }
        }
    }

    // Pass 2: emit row addresses and bytes.
    let mut rows = Vec::with_capacity(lines.len());
    pc = 0;
    for line in lines {
        let parsed = parse_listing_line(line);
        let text = map_c_source_marker(line.trim_end(), source_lines);

        let mut row = ListingRow {
            address: None,
            bytes: Vec::new(),
            text,
        };

        if let Some(stmt) = parsed.stmt {
            match stmt {
                ListingStmt::Org(v) => {
                    pc = v;
                    row.address = Some(pc);
                }
                _ => {
                    row.address = Some(pc);
                    row.bytes = stmt.encode(&labels, pc);
                    pc = pc.wrapping_add(row.bytes.len() as u16);
                }
            }
        } else if parsed.label.is_some() {
            row.address = Some(pc);
        }

        rows.push(row);
    }

    rows
}

fn map_c_source_marker(line: &str, source_lines: Option<&[String]>) -> String {
    let trimmed = line.trim_start();
    let marker = "; C_LINE ";
    if let Some(rest) = trimmed.strip_prefix(marker) {
        if let Ok(n) = rest.trim().parse::<usize>() {
            if let Some(src) = source_lines.and_then(|v| v.get(n.saturating_sub(1))) {
                return format!("; {} {}", n, src.trim_end());
            }
            return format!("; {}", n);
        }
    }
    line.to_string()
}

fn format_bytes_column(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return String::new();
    }
    let max_inline = 8usize;
    if bytes.len() <= max_inline {
        return bytes
            .iter()
            .map(|b| format!("{:02X}", b))
            .collect::<Vec<_>>()
            .join(" ");
    }
    let shown = bytes
        .iter()
        .take(max_inline)
        .map(|b| format!("{:02X}", b))
        .collect::<Vec<_>>()
        .join(" ");
    format!("{} ... (+{} bytes)", shown, bytes.len() - max_inline)
}

#[derive(Clone, Debug)]
struct ParsedListingLine {
    label: Option<String>,
    stmt: Option<ListingStmt>,
}

#[derive(Clone, Debug)]
enum ListingStmt {
    Org(u16),
    Db(Vec<String>),
    Dw(Vec<String>),
    Ds(usize),
    Inst { op: String, args: Vec<String> },
}

impl ListingStmt {
    fn size(&self, labels: &std::collections::HashMap<String, u16>) -> usize {
        self.encode(labels, 0).len()
    }

    fn encode(&self, labels: &std::collections::HashMap<String, u16>, pc: u16) -> Vec<u8> {
        match self {
            ListingStmt::Org(_) => Vec::new(),
            ListingStmt::Db(vals) => {
                let mut out = Vec::new();
                for v in vals {
                    out.push(eval_expr(v, labels, pc).unwrap_or(0) as u8);
                }
                out
            }
            ListingStmt::Dw(vals) => {
                let mut out = Vec::new();
                for v in vals {
                    let w = eval_expr(v, labels, pc).unwrap_or(0);
                    out.push((w & 0xFF) as u8);
                    out.push((w >> 8) as u8);
                }
                out
            }
            ListingStmt::Ds(n) => vec![0; *n],
            ListingStmt::Inst { op, args } => encode_8080_instruction(op, args, labels, pc),
        }
    }
}

fn parse_listing_line(line: &str) -> ParsedListingLine {
    let mut code = line;
    if let Some(idx) = line.find(';') {
        code = &line[..idx];
    }
    let mut code = code.trim();

    if code.is_empty() {
        return ParsedListingLine { label: None, stmt: None };
    }

    let mut label = None;
    if let Some(colon) = code.find(':') {
        let candidate = code[..colon].trim();
        if !candidate.is_empty() && is_identifier(candidate) {
            label = Some(candidate.to_string());
            code = code[colon + 1..].trim();
        }
    }

    if code.is_empty() {
        return ParsedListingLine { label, stmt: None };
    }

    let mut parts = code.splitn(2, char::is_whitespace);
    let op = parts.next().unwrap_or_default().trim();
    let op_upper = op.to_ascii_uppercase();
    let rest = parts.next().unwrap_or_default().trim();
    let args = split_args(rest);

    let stmt = match op_upper.as_str() {
        ".ORG" | "ORG" => Some(ListingStmt::Org(parse_u16_arg(args.first(), &std::collections::HashMap::new(), 0))),
        "DB" => Some(ListingStmt::Db(args)),
        "DW" => Some(ListingStmt::Dw(args)),
        "DS" | ".STORAGE" => {
            let n = parse_u16_arg(args.first(), &std::collections::HashMap::new(), 0) as usize;
            Some(ListingStmt::Ds(n))
        }
        _ => Some(ListingStmt::Inst {
            op: op_upper,
            args,
        }),
    };

    ParsedListingLine { label, stmt }
}

fn split_args(s: &str) -> Vec<String> {
    if s.is_empty() {
        return Vec::new();
    }
    s.split(',')
        .map(|a| a.trim().to_string())
        .filter(|a| !a.is_empty())
        .collect()
}

fn parse_u16_arg(
    arg: Option<&String>,
    labels: &std::collections::HashMap<String, u16>,
    pc: u16,
) -> u16 {
    arg.and_then(|a| eval_expr(a, labels, pc)).unwrap_or(0)
}

fn is_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c == '_' || c.is_ascii_alphabetic() || c == '.' => {}
        _ => return false,
    }
    chars.all(|c| c == '_' || c == '.' || c.is_ascii_alphanumeric())
}

fn eval_expr(expr: &str, labels: &std::collections::HashMap<String, u16>, pc: u16) -> Option<u16> {
    let mut total: i32 = 0;
    let mut sign: i32 = 1;
    let mut token = String::new();

    let flush = |tok: &str,
                 sign: i32,
                 total: &mut i32,
                 labels: &std::collections::HashMap<String, u16>,
                 pc: u16|
     -> Option<()> {
        let t = tok.trim();
        if t.is_empty() {
            return Some(());
        }
        let val = parse_atom(t, labels, pc)? as i32;
        *total += sign * val;
        Some(())
    };

    for ch in expr.chars() {
        if ch == '+' || ch == '-' {
            flush(&token, sign, &mut total, labels, pc)?;
            token.clear();
            sign = if ch == '+' { 1 } else { -1 };
        } else {
            token.push(ch);
        }
    }
    flush(&token, sign, &mut total, labels, pc)?;

    Some((total as i64 & 0xFFFF) as u16)
}

fn parse_atom(atom: &str, labels: &std::collections::HashMap<String, u16>, pc: u16) -> Option<u16> {
    let a = atom.trim();
    if a == "$" {
        return Some(pc);
    }
    if let Some(hex) = a.strip_prefix("0x") {
        return u16::from_str_radix(hex, 16).ok();
    }
    if let Some(hex) = a.strip_suffix('h').or_else(|| a.strip_suffix('H')) {
        return u16::from_str_radix(hex, 16).ok();
    }
    if let Some(bin) = a.strip_suffix('b').or_else(|| a.strip_suffix('B')) {
        if bin.chars().all(|c| c == '0' || c == '1') {
            return u16::from_str_radix(bin, 2).ok();
        }
    }
    if a.starts_with('"') && a.ends_with('"') && a.len() >= 2 {
        return a.as_bytes().get(1).copied().map(|b| b as u16);
    }
    if a.starts_with('\'') && a.ends_with('\'') && a.len() >= 3 {
        return a.as_bytes().get(1).copied().map(|b| b as u16);
    }
    if let Ok(v) = a.parse::<u16>() {
        return Some(v);
    }
    labels.get(a).copied()
}

fn reg_code(reg: &str) -> Option<u8> {
    match reg.trim().to_ascii_uppercase().as_str() {
        "B" => Some(0),
        "C" => Some(1),
        "D" => Some(2),
        "E" => Some(3),
        "H" => Some(4),
        "L" => Some(5),
        "M" => Some(6),
        "A" => Some(7),
        _ => None,
    }
}

fn rp_code(rp: &str) -> Option<u8> {
    match rp.trim().to_ascii_uppercase().as_str() {
        "B" | "BC" => Some(0),
        "D" | "DE" => Some(1),
        "H" | "HL" => Some(2),
        "SP" => Some(3),
        _ => None,
    }
}

fn push_pop_code(rp: &str) -> Option<u8> {
    match rp.trim().to_ascii_uppercase().as_str() {
        "B" | "BC" => Some(0),
        "D" | "DE" => Some(1),
        "H" | "HL" => Some(2),
        "PSW" | "A" => Some(3),
        _ => None,
    }
}

fn encode_imm16(op: u8, value: u16) -> Vec<u8> {
    vec![op, (value & 0xFF) as u8, (value >> 8) as u8]
}

fn encode_8080_instruction(
    op: &str,
    args: &[String],
    labels: &std::collections::HashMap<String, u16>,
    pc: u16,
) -> Vec<u8> {
    let imm8 = |idx: usize| -> u8 {
        args.get(idx)
            .and_then(|a| eval_expr(a, labels, pc))
            .unwrap_or(0) as u8
    };
    let imm16 = |idx: usize| -> u16 {
        args.get(idx)
            .and_then(|a| eval_expr(a, labels, pc))
            .unwrap_or(0)
    };

    match op {
        "NOP" => vec![0x00],
        "HLT" => vec![0x76],
        "DI" => vec![0xF3],
        "EI" => vec![0xFB],
        "RLC" => vec![0x07],
        "RRC" => vec![0x0F],
        "RAL" => vec![0x17],
        "RAR" => vec![0x1F],
        "DAA" => vec![0x27],
        "CMA" => vec![0x2F],
        "STC" => vec![0x37],
        "CMC" => vec![0x3F],
        "RET" => vec![0xC9],
        "RNZ" => vec![0xC0],
        "RZ" => vec![0xC8],
        "RNC" => vec![0xD0],
        "RC" => vec![0xD8],
        "RPO" => vec![0xE0],
        "RPE" => vec![0xE8],
        "RP" => vec![0xF0],
        "RM" => vec![0xF8],
        "PCHL" => vec![0xE9],
        "XTHL" => vec![0xE3],
        "XCHG" => vec![0xEB],
        "SPHL" => vec![0xF9],
        "RIM" => vec![0x20],
        "SIM" => vec![0x30],

        "MOV" => {
            let dst = args.first().and_then(|a| reg_code(a)).unwrap_or(0);
            let src = args.get(1).and_then(|a| reg_code(a)).unwrap_or(0);
            vec![0x40 + dst * 8 + src]
        }
        "MVI" => {
            let r = args.first().and_then(|a| reg_code(a)).unwrap_or(0);
            vec![0x06 + r * 8, imm8(1)]
        }
        "INR" => {
            let r = args.first().and_then(|a| reg_code(a)).unwrap_or(0);
            vec![0x04 + r * 8]
        }
        "DCR" => {
            let r = args.first().and_then(|a| reg_code(a)).unwrap_or(0);
            vec![0x05 + r * 8]
        }
        "ADD" => {
            let r = args.first().and_then(|a| reg_code(a)).unwrap_or(0);
            vec![0x80 + r]
        }
        "ADC" => {
            let r = args.first().and_then(|a| reg_code(a)).unwrap_or(0);
            vec![0x88 + r]
        }
        "SUB" => {
            let r = args.first().and_then(|a| reg_code(a)).unwrap_or(0);
            vec![0x90 + r]
        }
        "SBB" => {
            let r = args.first().and_then(|a| reg_code(a)).unwrap_or(0);
            vec![0x98 + r]
        }
        "ANA" => {
            let r = args.first().and_then(|a| reg_code(a)).unwrap_or(0);
            vec![0xA0 + r]
        }
        "XRA" => {
            let r = args.first().and_then(|a| reg_code(a)).unwrap_or(0);
            vec![0xA8 + r]
        }
        "ORA" => {
            let r = args.first().and_then(|a| reg_code(a)).unwrap_or(0);
            vec![0xB0 + r]
        }
        "CMP" => {
            let r = args.first().and_then(|a| reg_code(a)).unwrap_or(0);
            vec![0xB8 + r]
        }

        "LXI" => {
            let rp = args.first().and_then(|a| rp_code(a)).unwrap_or(0);
            encode_imm16(0x01 + rp * 0x10, imm16(1))
        }
        "INX" => {
            let rp = args.first().and_then(|a| rp_code(a)).unwrap_or(0);
            vec![0x03 + rp * 0x10]
        }
        "DCX" => {
            let rp = args.first().and_then(|a| rp_code(a)).unwrap_or(0);
            vec![0x0B + rp * 0x10]
        }
        "DAD" => {
            let rp = args.first().and_then(|a| rp_code(a)).unwrap_or(0);
            vec![0x09 + rp * 0x10]
        }
        "LDAX" => match args.first().map(|a| a.trim().to_ascii_uppercase()) {
            Some(ref s) if s == "B" || s == "BC" => vec![0x0A],
            Some(ref s) if s == "D" || s == "DE" => vec![0x1A],
            _ => vec![0x0A],
        },
        "STAX" => match args.first().map(|a| a.trim().to_ascii_uppercase()) {
            Some(ref s) if s == "B" || s == "BC" => vec![0x02],
            Some(ref s) if s == "D" || s == "DE" => vec![0x12],
            _ => vec![0x02],
        },
        "PUSH" => {
            let rp = args.first().and_then(|a| push_pop_code(a)).unwrap_or(0);
            vec![0xC5 + rp * 0x10]
        }
        "POP" => {
            let rp = args.first().and_then(|a| push_pop_code(a)).unwrap_or(0);
            vec![0xC1 + rp * 0x10]
        }

        "ADI" => vec![0xC6, imm8(0)],
        "ACI" => vec![0xCE, imm8(0)],
        "SUI" => vec![0xD6, imm8(0)],
        "SBI" => vec![0xDE, imm8(0)],
        "ANI" => vec![0xE6, imm8(0)],
        "XRI" => vec![0xEE, imm8(0)],
        "ORI" => vec![0xF6, imm8(0)],
        "CPI" => vec![0xFE, imm8(0)],

        "JMP" => encode_imm16(0xC3, imm16(0)),
        "JNZ" => encode_imm16(0xC2, imm16(0)),
        "JZ" => encode_imm16(0xCA, imm16(0)),
        "JNC" => encode_imm16(0xD2, imm16(0)),
        "JC" => encode_imm16(0xDA, imm16(0)),
        "JPO" => encode_imm16(0xE2, imm16(0)),
        "JPE" => encode_imm16(0xEA, imm16(0)),
        "JP" => encode_imm16(0xF2, imm16(0)),
        "JM" => encode_imm16(0xFA, imm16(0)),

        "CALL" => encode_imm16(0xCD, imm16(0)),
        "CNZ" => encode_imm16(0xC4, imm16(0)),
        "CZ" => encode_imm16(0xCC, imm16(0)),
        "CNC" => encode_imm16(0xD4, imm16(0)),
        "CC" => encode_imm16(0xDC, imm16(0)),
        "CPO" => encode_imm16(0xE4, imm16(0)),
        "CPE" => encode_imm16(0xEC, imm16(0)),
        "CP" => encode_imm16(0xF4, imm16(0)),
        "CM" => encode_imm16(0xFC, imm16(0)),

        "LDA" => encode_imm16(0x3A, imm16(0)),
        "STA" => encode_imm16(0x32, imm16(0)),
        "LHLD" => encode_imm16(0x2A, imm16(0)),
        "SHLD" => encode_imm16(0x22, imm16(0)),
        "IN" => vec![0xDB, imm8(0)],
        "OUT" => vec![0xD3, imm8(0)],
        "RST" => {
            let n = (imm16(0) & 0x7) as u8;
            vec![0xC7 + (n << 3)]
        }

        _ => Vec::new(),
    }
}

fn build_full_asm(code_lines: &[String]) -> Vec<String> {
    let mut lines = Vec::new();

    // Header
    lines.push("; Generated by v6c compiler".to_string());
    lines.push(String::new());

    // Origin
    lines.push("\t.org 0x100".to_string());
    lines.push(String::new());

    // Entry point — jump to CRT0 startup
    lines.push("\tJMP _start".to_string());
    lines.push(String::new());

    // CRT0 startup code
    lines.push("; --- crt0 startup ---".to_string());
    for line in runtime::crt0_asm().lines() {
        // Skip comment-only header lines already in the source
        if line.starts_with(';') && !line.contains("_start") {
            continue;
        }
        lines.push(line.to_string());
    }
    lines.push(String::new());

    // Code from the code generator
    lines.extend(code_lines.iter().cloned());

    // Runtime library — include only the modules that are referenced
    lines.push(String::new());
    lines.push("; --- runtime library ---".to_string());
    let rt = runtime::collect_runtime(code_lines);
    if !rt.is_empty() {
        lines.extend(rt.lines().map(|l| l.to_string()));
    }

    lines
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    /// Helper: emit into a uniquely-named file and return its contents.
    fn emit_and_read(lines: &[String]) -> String {
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = format!("test_emit_output_{}.asm", id);
        emit_asm(lines, &path).expect("emit_asm failed");
        let contents = std::fs::read_to_string(&path).expect("read failed");
        let _ = std::fs::remove_file(&path);
        contents
    }

    #[test]
    fn header_and_org_present() {
        let out = emit_and_read(&[]);
        assert!(out.starts_with("; Generated by v6c compiler"));
        assert!(out.contains("\t.org 0x100"));
    }

    #[test]
    fn entry_point_jump() {
        let out = emit_and_read(&[]);
        assert!(out.contains("\tJMP _start"));
    }

    #[test]
    fn crt0_startup_present() {
        let out = emit_and_read(&[]);
        assert!(out.contains("_start:"));
        assert!(out.contains("LXI SP"));
        assert!(out.contains("CALL main"));
        assert!(out.contains("HLT"));
    }

    #[test]
    fn code_lines_emitted() {
        let lines = vec![
            "main:".to_string(),
            "\tMVI A,0".to_string(),
            "\tRET".to_string(),
        ];
        let out = emit_and_read(&lines);
        assert!(out.contains("main:"));
        assert!(out.contains("\tMVI A,0"));
        assert!(out.contains("\tRET"));
    }

    #[test]
    fn runtime_marker_at_end() {
        let out = emit_and_read(&[]);
        assert!(out.contains("; --- runtime library ---"));
    }

    #[test]
    fn lst_contains_addr_bytes_and_source_columns() {
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = format!("test_emit_output_{}.lst", id);
        let lines = vec!["main:".to_string(), "\tRET".to_string()];

        emit_lst(&lines, &path, None).expect("emit_lst failed");
        let contents = std::fs::read_to_string(&path).expect("read failed");
        let _ = std::fs::remove_file(&path);

        assert!(
            contents.lines().next().unwrap_or_default().contains("ADDR"),
            "listing header must contain address column"
        );
        assert!(
            contents.contains("0100") && contents.contains("C3") && contents.contains("JMP _start"),
            "listing must show address and emitted bytes for instructions"
        );
        assert!(contents.contains("main:"));
        assert!(contents.contains("\tRET"));
    }

    #[test]
    fn lst_renders_c_line_markers_with_source_text() {
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = format!("test_emit_output_{}.lst", id);
        let lines = vec![
            "; C_LINE 3".to_string(),
            "main:".to_string(),
            "\tRET".to_string(),
        ];
        let source = vec![
            "int a;".to_string(),
            "int b;".to_string(),
            "int main(void) { return 0; }".to_string(),
        ];

        emit_lst(&lines, &path, Some(&source)).expect("emit_lst failed");
        let contents = std::fs::read_to_string(&path).expect("read failed");
        let _ = std::fs::remove_file(&path);

        assert!(contents.contains("; 3 int main(void) { return 0; }"));
    }

    #[test]
    fn runtime_included_when_needed() {
        let lines = vec![
            "main:".to_string(),
            "\tCALL __mul16".to_string(),
            "\tRET".to_string(),
        ];
        let out = emit_and_read(&lines);
        assert!(out.contains("__mul16:"), "runtime __mul16 should be included");
    }

    #[test]
    fn order_is_correct() {
        let lines = vec!["\tNOP".to_string()];
        let out = emit_and_read(&lines);

        let org_pos = out.find("\t.org 0x100").unwrap();
        let jmp_pos = out.find("\tJMP _start").unwrap();
        let nop_pos = out.find("\tNOP").unwrap();
        let rt_pos = out.find("; --- runtime library ---").unwrap();

        assert!(org_pos < jmp_pos);
        assert!(jmp_pos < nop_pos);
        assert!(nop_pos < rt_pos);
    }
}
