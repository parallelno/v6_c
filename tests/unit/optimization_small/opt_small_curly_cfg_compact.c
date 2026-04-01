// opt_small_curly_cfg_compact.c - compact CFG
//
// Feature: merge adjacent blocks where possible and remove unnecessary jumps.
// Benefit: reduces instruction count and improves fall-through density.

int k;

int main(void) {
    k = 1;
    if (k == 1) {
        k = 2;  /* k=2 */
    }
    if (k != 2) return 1;
    return 0;
}
