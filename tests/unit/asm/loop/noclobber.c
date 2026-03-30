/* Feature: Empty-param asm block (no clobber) inside a loop. */
/* Benefit:  Proves that asm(){} with no parameters does not disturb */
/*           the register allocator, so loop variables stay in registers. */
/* Method:   noclobber_in_loop() sums 1..10 with asm(){NOP} each */
/*           iteration. If NOP clobbered state the sum would be wrong. */
/* Expect:   Compiles without error. NOP survives the peephole optimizer. */

int result;

void noclobber_in_loop() {
    int i;
    int sum;
    sum = 0;
    for (i = 1; i <= 10; i = i + 1) {
        asm() {
            NOP
        };
        sum = sum + i;
    }
    result = sum;
}

void main(void) {
    noclobber_in_loop();
}
