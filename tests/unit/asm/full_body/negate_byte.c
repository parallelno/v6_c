/* Feature: Full-body asm function operating on a char (8-bit) parameter. */
/* Benefit:  Verifies that char-sized args are correctly placed in the A */
/*           register and that the compiler adds an auto-RET. */
/* Method:   negate_byte(char x) uses CMA/INR A to negate via twos */
/*           complement. Called twice to prevent inlining. */
/* Expect:   Compiles without error. Output contains CMA and INR A. */

char negate_byte(char x) {
    asm {
        CMA
        INR A
    }
}

int main(void) {
    if (negate_byte(5) != -5) return 1;
    if (negate_byte(10) != -10) return 2;
    return 0;
}
