// opt_small_dead_branch.c - dead branch elimination
//
// Feature: remove conditional branches whose condition is a compile-time constant.
// Benefit: eliminates unreachable code and reduces instruction count.

int x;

int main(void) {
    if (0) {
        x = 1; /* unreachable */
    }
    if (1) {
        x = 2; /* always taken */
    }
    if (x != 2) return 1;
    return 0;
}
