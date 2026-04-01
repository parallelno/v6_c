// opt_small_cse.c - common subexpression elimination (CSE)
//
// Feature: detect repeated expressions and avoid duplicate work.
// Benefit: reduce duplicated instructions and improve register pressure.

int z;

int main(void) {
    int a = 3;
    int b = 4;
    z = (a + b) * (a + b);  /* 7*7 = 49 */
    if (z != 49) return 1;
    return 0;
}
