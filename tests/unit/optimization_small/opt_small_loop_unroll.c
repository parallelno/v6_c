// opt_small_loop_unroll.c - loop unrolling hint
//
// Feature: expand small constant-trip-count loops when hint is present.
// Benefit: fewer branch checks and more straight-line code.
// Example:
//   for (int i=0; i<3; i++) sum += i;

int sum;

void main(void) {
    sum = 0;
    for (int i = 0; i < 3; i++) {
        sum += i;
    }
}
