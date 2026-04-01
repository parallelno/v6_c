// opt_small_dead_store_elimination.c - dead store elimination
//
// Feature: remove stores to memory that are not later read.
// Benefit: avoids unnecessary instructions and reduces code size.

int a;
int b;

int main(void) {
    a = 5;   /* dead store */
    a = 6;   /* live */
    b = a;   /* b=6 */
    if (a != 6) return 1;
    if (b != 6) return 2;
    return 0;
}
