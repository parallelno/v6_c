// opt_small_const_fold.c - constant folding
//
// Feature: constant folding of pure arithmetic expressions.
// Benefit: reduces instruction count and removes runtime computation.

int x;
int y;

int main(void) {
    x = (2 + 3) * 4;  /* 20 */
    y = 8 / 2;         /* 4 */
    if (x != 20) return 1;
    if (y != 4) return 2;
    return 0;
}
