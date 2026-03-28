// opt_small_remove_redundant_moves.c - redundant move elimination
//
// Feature: remove move/copy operations that are unnecessary.
// Benefit: simplified code path and fewer instructions.
// Example:
//   a = b; a = b; // second assignment is redundant after optimization

int a;
int b;

void main(void) {
    b = 5;
    a = b;
    a = b;
}
