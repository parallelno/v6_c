Develop a professional design document for a floating point support for the Vector 06c (Intel 8080 (8-bit) compatible machine).

Key Technical Requirements:
The target system has a 64KB flat memory model.
Intel 8080 (8-bit) CPU with specific Vector 06c instruction timings.

MOV R1,R     8       MOV R,M    8        MVI R,D8  8
MVI M,D8    12       STAX RP    8        LDAX RP   8
STA ADR     16       LDA ADR    16
LXI RP,D16  12       SHLD ADR   20        LHLD ADR  20
PUSH RP     16       POP RP     12        SPHL      8
XCHG        4        XTHL       24
CMC         4        STC        4        CMA       4
DAA         4        INR R      8        INR M     12
DCR R       8        DCR M      12       INX RP    8
DCX RP      8
ADD R       4        ADD M      8        SUB R     4
SUB M       8        ADC R      4        ADC M     8
SBB R       4        SBB M      8        ANA R     4
ANA M       8        ORA R      4        ORA M     8
XRA R       4        XRA M      8        ADI D8    8
ACI D8      8        SUI D8     8        SBI D8    8
ANI D8      8        ORI D8     8        XRI D8    8
CMP R       4        CMP M      8        CPI D8    8
DAD RP      12
RRC         4        RLC        4        RAL       4
RAR         4
EI          4        DI         4        NOP       4
HLT         8
IN #        12       OUT #     12
PCHL        8        JMP ADR    12        J* ADR   12
CALL ADR    24       C* ADR /Y 24        C* ADR /N 16
RST N       16       RET       12        R*     /Y 16
R*      /N  8

| Instruction  | Note |
|-------------|------|
| MOV r,r      | Twice slower than ALU ops — makes register shuffling costlier |
| MVI r,n      | Nearly free difference vs MOV |
| INR/DCR r    | Same cost as MOV on V6C |
| INX/DCX rp   | Same cost as INR on V6C |
| ADD/SUB/AND/OR/XOR r | ALU ops are the cheapest class |
| ADI/SUI/ANI/ORI/XRI n | Immediate ALU ties with reg MVI |
| DAD rp       | Relatively cheap 16-bit add |
| LXI rp,nn    | Same cost as DAD |
| MOV r,M / MOV M,r | Memory access = register move cost |
| LDA/STA addr | Low Direct memory = 2× MOV cost |
| LHLD/SHLD    | Slower then Push/Pop |
| PUSH rp      | Cheap for memcpy, memset, etc — critical for SP-based tricks |
| POP rp       | Cheap |
| XCHG         | Free — same as ALU op |
| ORA r        | Preferred over CPI 0 (saves 4cc) |
| RRC, RAR, RLC, RAL | cheapest ALU ops. Great for bits manipulation |

Use tooling:
* v6asm.exe
    Purpose: for tests
    Path: tools\v6asm\v6asm.exe
    Docs: tools\v6asm\docs\README.md
* v6emul.exe
   Purpose: for testing
   Path: tools\v6emul\v6emul.exe
   Docs: tools\v6emul\docs\README.md
   Helpful CLI commands:
      | `--rom <path>` | string | *(none)* | Path to a ROM file to load into emulator memory |
      | `--load-addr <addr>` | int | `0` | Memory address to load the ROM at. Supports hex (`0x100`) and decimal (`256`) |
      | `--halt-exit` | flag | — | Exit on first HLT instruction |
      | `--speed <speed>` | string | *(normal)* | Execution speed: `max` |
      | `--dump-cpu` | flag | — | Print full CPU state on exit (registers, flags, PC, SP, cycles) |
      | `--dump-memory` | flag | — | Print full 64K memory dump (hex) on exit |
    Example: v6emul --rom test.rom --halt-exit --dump-cpu --load-addr 0


The default starting execution address is 0x100, but you can use 0x00 for testing purposes.

Solution: ASM highly optimized for I8080 (8 bit) floating point routines. To make the process less risky, you can start with the C implementation, and then replace it with ASM.
Optimization: Because 8-bit registers are scarce, lean heavily on ASM code optimization to minimize memory access and register pressure.

Act as an experienced software architect.
Explore the existing codebase. Discover and validate the existing ASM related information including code, tests, docs.
Create a high-level design for a complex software funtionality that prioritizes modularity, robustness, maintainability, documentation quality, and test coverage.
Limit the output to design concepts, architectural decisions, and interfaces.
Do not include implementation details or step-by-step development instructions.