/* Feature: Raw asm block accessing local variables via _l_ labels. */
/* Benefit:  Allows hand-written assembly to load and store local variables */
/*           using the compiler naming convention _l_<func>_<var>. */
/* Method:   sum_via_asm(int a, int b) uses asm{} to LHLD both locals, */
/*           add them with DAD D, and SHLD the result into a local sum. */
/* Expect:   Compiles without error. Output references _l_sum_via_asm_a. */

int sum_via_asm(int a, int b) {
    int sum;
    sum = 0;
    asm {
        LHLD _l_sum_via_asm_a
        XCHG
        LHLD _l_sum_via_asm_b
        DAD D
        SHLD _l_sum_via_asm_sum
    }
    return sum;
}

int main(void) {
    if (sum_via_asm(30, 12) != 42) return 1;
    return 0;
}
