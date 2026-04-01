// opt_small_loop_unroll.c - loop unrolling hint
//
// Feature: expand small constant-trip-count loops when hint is present.
// Benefit: fewer branch checks and more straight-line code.

int sum;

int main(void) {
    sum = 0;
#pragma unroll
    for (int i = 0; i < 3; i++) {
        sum += i;  /* 0+1+2 = 3 */
    }
    if (sum != 3) return 1;
    return 0;
}
