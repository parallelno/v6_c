// opt_small_cse.c - common subexpression elimination (CSE)
//
// Feature: detect repeated expressions and avoid duplicate work.
// Benefit: reduce duplicated instructions and improve register pressure.
// Example:
//   z = (a + b) * (a + b); // compute a+b once

int z;

void main(void) {
    int a = 3;
    int b = 4;
    z = (a + b) * (a + b);
}
