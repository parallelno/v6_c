// opt_small_strength_reduction.c - strength reduction
//
// Feature: convert expensive operations to cheaper ones (mul->add/shift).
// Benefit: reduce cycle cost for repeated computations.
// Example:
//   y = x * 4; // becomes shift left by 2

int x;
int y;

void main(void) {
    x = 7;
    y = x * 4;
}
