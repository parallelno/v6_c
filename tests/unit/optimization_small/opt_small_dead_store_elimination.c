// opt_small_dead_store_elimination.c - dead store elimination
//
// Feature: remove stores to memory that are not later read.
// Benefit: avoids unnecessary instructions and reduces code size.
// Example:
//   a = 5; a = 6; return a; // first store is dead and can be removed.

int a;
int b;

void main(void) {
    a = 5;
    a = 6; // first value is dead
    b = a;
}
