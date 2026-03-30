/* Feature: Raw asm block inside nested loops. */
/* Benefit:  Stress-tests register spill/restore and asm-as-barrier */
/*           semantics when the asm block executes in an inner loop. */
/* Method:   nested_loop() has for(i<3) { for(j<2) { asm{} } } where */
/*           each asm block increments _g_result via LHLD/INX H/SHLD. */
/* Expect:   Compiles without error. */

int result;

void nested_loop() {
    int i;
    int j;
    result = 0;
    for (i = 0; i < 3; i = i + 1) {
        for (j = 0; j < 2; j = j + 1) {
            asm {
                LHLD _g_result
                INX H
                SHLD _g_result
            }
        }
    }
}

void main(void) {
    nested_loop();
}
