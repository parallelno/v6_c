// opt_small_inline_specialize.c - inline specialization
//
// Feature: inline small known functions and optimize parameter constants.
// Benefit: removes call overhead and enables further peephole optimization.

int y;

int add1(int x) {
    return x + 1;
}

int main(void) {
    y = add1(3);  /* 4 */
    if (y != 4) return 1;
    return 0;
}
