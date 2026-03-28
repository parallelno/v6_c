// opt_small_cse.c - common subexpression elimination (CSE)
//
// Feature: detect repeated expressions and avoid duplicate work.
// Benefit: reduce duplicated instructions and improve register pressure.
// Example:
//   z = (a + b) * (a + b); // compute a+b once

int a;
int b;
int z;

void main(void) {
    a = 3;
    b = 4;
    z = (a + b) * (a + b);
}
