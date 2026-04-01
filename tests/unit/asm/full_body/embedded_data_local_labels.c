/* Feature: Full-body asm function with embedded data (.DW directive). */
/* Benefit:  Shows that assembler directives like .DW can be placed */
/*           inside an asm body, enabling lookup tables and constants. */
/* Method:   load_const() uses LHLD to read a .DW word placed after a */
/*           RET, with a _lc_data: label marking the data location. */
/* Expect:   Compiles without error. Output contains the _lc_data: label. */

int load_const() {
    asm {
        LHLD @data
        RET
@data:
        .DW 0x1234
    }
}
int load_const2() {
    asm {
        LHLD @data
        RET
@data:
        .DW 0x5678
    }
}

int main(void) {
    if (load_const() != 0x1234) return 1;
    if (load_const2() != 0x5678) return 2;
    return 0;
}
