// opt_small_curly_cfg_compact.c - compact CFG
//
// Feature: merge adjacent blocks where possible and remove unnecessary jumps.
// Benefit: reduces instruction count and improves fall-through density.
// Example:
//   label1: jmp label2; label2: ... -> direct fall-through or remove noop jump.

int k;

void main(void) {
    k = 1;
    if (k == 1) {
        k = 2;
    }
}
