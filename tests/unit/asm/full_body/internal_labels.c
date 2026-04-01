/* Feature: Full-body asm function with internal labels and jumps. */
/* Benefit:  Proves that user-defined labels inside an asm block pass */
/*           through to the output unmodified and can be jump targets. */
/* Method:   abs_val(int x) branches to _abs_done to skip negation when */
/*           the sign bit is clear. */
/* Expect:   Compiles without error. Output contains the _abs_done: label. */

int abs_val(int x) {
    asm {
        MOV A,H
        ORA A
        JP _abs_done
        ; negate HL
        MOV A,H
        CMA
        MOV H,A
        MOV A,L
        CMA
        MOV L,A
        INX H
_abs_done:
    }
}

int main(void) {
    if (abs_val(-5) != 5) return 1;
    if (abs_val(5) != 5) return 2;
    return 0;
}
