// opt_small_remove_redundant_moves.c - redundant move elimination
//
// Feature: remove move/copy operations that are unnecessary.
// Benefit: simplified code path and fewer instructions.

int a;
int b;

int main(void) {
    b = 5;
    a = b;  /* a=5 */
    a = b;  /* redundant: a still 5 */
    if (a != 5) return 1;
    if (b != 5) return 2;
    return 0;
}
