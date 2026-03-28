// opt_small_dead_branch.c - dead branch elimination
//
// Feature: remove conditional branches whose condition is a compile-time constant.
// Benefit: eliminates unreachable code and reduces instruction count.
// Example:
//   if (0) { x = 1; }  -> entire branch removed; body is unreachable.
//   if (1) { x = 2; }  -> branch removed; body always executes inline.

int x;

void main(void) {
    if (0) {
        x = 1; // unreachable — whole branch must be removed
    }
    if (1) {
        x = 2; // always taken — branch condition and jump must be removed
    }
}
