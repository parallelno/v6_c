/* Feature: Full-body asm function with embedded data (.DW directive). */
/* Benefit:  Shows that assembler directives like .DW can be placed */
/*           inside an asm body, enabling lookup tables and constants. */
/* Method:   load_const() uses LHLD to read a .DW word placed after a */
/*           RET, with a _lc_data: label marking the data location. */
/* Expect:   Compiles without error. Output contains the _lc_data: label. */

int result;

int load_const() {
    asm {
        LHLD _lc_data
        RET
_lc_data:
        .DW 0x1234
    }
}

void main(void) {
    result = load_const();
}
