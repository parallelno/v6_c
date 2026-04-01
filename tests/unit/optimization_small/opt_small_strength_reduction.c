// opt_small_strength_reduction.c - strength reduction
//
// Feature: convert expensive operations to cheaper ones (mul->add/shift).
// Benefit: reduce cycle cost for repeated computations.

int x;
int y;

int main(void) {
    x = 7;
    y = x * 4;  /* 28 */
    if (y != 28) return 1;
    return 0;
}
