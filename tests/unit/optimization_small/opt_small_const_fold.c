// opt_small_const_fold.c - constant folding
//
// Feature: constant folding of pure arithmetic expressions.
// Benefit: reduces instruction count and removes runtime computation.
// Example:
//   x = (2 + 3) * 4; // becomes x = 20
//   y = 8 / 2;      // becomes y = 4
//
// This test verifies compile-time evaluation of operations on constants.

int x;
int y;

void main(void) {
    x = (2 + 3) * 4;
    y = 8 / 2;
}
