/* Feature: Parameterized asm block inside a for loop. */
/* Benefit:  Confirms that selective register spill and restore works */
/*           correctly when the asm block re-executes across iterations. */
/* Method:   param_in_loop() passes a loop-derived char value into */
/*           asm(char val) { OUT 1 } on each of three iterations. */
/* Expect:   Compiles without error. */

void param_in_loop() {
    int i;
    for (i = 0; i < 3; i = i + 1) {
        char val;
        val = i;
        asm(char val) {
            OUT 1
        };
    }
}

void main(void) {
    param_in_loop();
}
